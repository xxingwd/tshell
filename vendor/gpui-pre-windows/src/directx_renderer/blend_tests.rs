//! Exercise the production blend states on D3D11 WARP and read back pixels.
//! This catches destination-alpha errors that opaque screenshots conceal.
use super::*;
use windows::{Win32::Graphics::Direct3D::Fxc::D3DCompile, core::s};

const SHADER: &str = r#"
cbuffer Ink : register(b0) { float4 ink; }
float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}
float4 ps() : SV_Target { return ink; }
"#;

unsafe fn compile(entry: windows::core::PCSTR, target: windows::core::PCSTR) -> Result<Vec<u8>> {
    let mut blob = None;
    unsafe {
        D3DCompile(
            SHADER.as_ptr().cast(),
            SHADER.len(),
            None,
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut blob,
            None,
        )?;
        let blob = blob.unwrap();
        Ok(slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize()).to_vec())
    }
}

#[::core::prelude::v1::test]
fn transparent_ink_matches_source_over_gpu_readback() -> Result<()> {
    // WARP runs the real Direct3D blend pipeline without a window or a GPU
    // vendor dependency. The two factories below are used by GPUI at runtime.
    unsafe {
        let mut device = None;
        let mut context = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_WARP,
            Default::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
        let device = device.unwrap();
        let context = context.unwrap();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 4,
            Height: 4,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let mut target = None;
        device.CreateTexture2D(&desc, None, Some(&mut target))?;
        let target = target.unwrap();
        let mut view = None;
        device.CreateRenderTargetView(&target, None, Some(&mut view))?;
        let view = view.unwrap();
        let mut staging = None;
        device.CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..desc
            },
            None,
            Some(&mut staging),
        )?;
        let staging = staging.unwrap();
        let mut constant = None;
        device.CreateBuffer(
            &D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            },
            None,
            Some(&mut constant),
        )?;
        let constant = constant.unwrap();
        let vs = create_vertex_shader(&device, &compile(s!("vs"), s!("vs_5_0"))?)?;
        let ps = create_fragment_shader(&device, &compile(s!("ps"), s!("ps_5_0"))?)?;
        context.VSSetShader(&vs, None);
        context.PSSetShader(&ps, None);
        context.PSSetConstantBuffers(0, Some(&[Some(constant.clone())]));
        context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        context.OMSetRenderTargets(Some(&[Some(view.clone())]), None);
        context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            Width: 4.,
            Height: 4.,
            MaxDepth: 1.,
            ..Default::default()
        }]));

        let quantize = |v: f32| (v * 255.).round() / 255.;
        for premultiplied in [false, true] {
            let blend = if premultiplied {
                create_blend_state_for_path_sprite(&device)?
            } else {
                create_blend_state(&device)?
            };
            context.OMSetBlendState(&blend, None, u32::MAX);
            for background in [[1., 1., 1.], [16. / 255., 22. / 255., 34. / 255.]] {
                for background_alpha in [0., 0.2, 0.5, 0.8, 1.] {
                    for ink_alpha in [0., 0.25, 0.5, 1.] {
                        let clear = [
                            background[0] * background_alpha,
                            background[1] * background_alpha,
                            background[2] * background_alpha,
                            background_alpha,
                        ];
                        let blue = [36. / 255., 92. / 255., 190. / 255.];
                        let multiplier = if premultiplied { ink_alpha } else { 1. };
                        let ink = [
                            blue[0] * multiplier,
                            blue[1] * multiplier,
                            blue[2] * multiplier,
                            ink_alpha,
                        ];
                        context.ClearRenderTargetView(&view, &clear);
                        context.UpdateSubresource(&constant, 0, None, ink.as_ptr().cast(), 0, 0);
                        context.Draw(3, 0);
                        context.CopyResource(&staging, &target);
                        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                        context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                        let pixel: [u8; 4] = slice::from_raw_parts(mapped.pData.cast(), 4)
                            .try_into()
                            .unwrap();
                        context.Unmap(&staging, 0);
                        // Porter-Duff source-over in a premultiplied target.
                        let expected = [
                            blue[0] * ink_alpha + quantize(clear[0]) * (1. - ink_alpha),
                            blue[1] * ink_alpha + quantize(clear[1]) * (1. - ink_alpha),
                            blue[2] * ink_alpha + quantize(clear[2]) * (1. - ink_alpha),
                            ink_alpha + quantize(background_alpha) * (1. - ink_alpha),
                        ]
                        .map(|v| (v * 255.).round() as u8);
                        for channel in 0..4 {
                            assert!(
                                pixel[channel].abs_diff(expected[channel]) <= 1,
                                "premultiplied={premultiplied} bg={background:?} bg_alpha={background_alpha} ink_alpha={ink_alpha}: actual={pixel:?}, expected={expected:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
