use gpui_kit::{Bounds, Pixels};
use std::{
    borrow::Cow,
    sync::{
        Arc, RwLock,
        atomic::{AtomicU8, Ordering},
    },
};
use wry::http::{Request, Response, StatusCode, header};

const PAGE_URL: &str = "http://tshell-preview.localhost/document.html";
const PDF_URL: &str = "http://tshell-pdf.localhost/document.pdf";

#[derive(Clone)]
pub enum WebPreviewSource {
    Html(String),
    Graphic(String),
    Pdf(Arc<Vec<u8>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    Loading,
    Ready,
    Failed,
}

pub struct WebPreview {
    webview: wry::WebView,
    html: String,
    page: Arc<RwLock<String>>,
    pdf: bool,
    graphic: bool,
    status: Arc<AtomicU8>,
    revision: u64,
    bounds: Bounds<Pixels>,
    visible: bool,
}

impl WebPreview {
    pub(crate) fn new(
        source: WebPreviewSource,
        bounds: Bounds<Pixels>,
        parent: wry::raw_window_handle::RawWindowHandle,
    ) -> wry::Result<Self> {
        // WebView2 pumps native messages here; no GPUI borrow may span construction.
        let parent = unsafe { wry::raw_window_handle::WindowHandle::borrow_raw(parent) };
        let status = Arc::new(AtomicU8::new(0));
        let page = Arc::new(RwLock::new(String::new()));
        let pdf = matches!(source, WebPreviewSource::Pdf(_));
        let graphic = matches!(source, WebPreviewSource::Graphic(_));
        let (html, builder) = match source {
            WebPreviewSource::Html(html) | WebPreviewSource::Graphic(html) => {
                *page.write().unwrap_or_else(|error| error.into_inner()) = html.clone();
                let shared_page = page.clone();
                let builder = wry::WebViewBuilder::new()
                    .with_custom_protocol("tshell-preview".into(), move |_, request| {
                        page_response(&request, &shared_page)
                    })
                    .with_url(PAGE_URL);
                (html, builder)
            }
            WebPreviewSource::Pdf(bytes) => {
                let builder = wry::WebViewBuilder::new()
                    .with_custom_protocol("tshell-pdf".into(), move |_, request| {
                        pdf_response(&request, &bytes)
                    })
                    .with_url(PDF_URL);
                (String::new(), builder)
            }
        };
        let loaded_status = status.clone();
        let render_status = status.clone();
        let webview = builder
            .with_visible(false)
            .with_incognito(true)
            .with_navigation_handler(move |url| navigation_allowed(&url, pdf))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_on_page_load_handler(move |event, url| {
                if !graphic
                    && matches!(event, wry::PageLoadEvent::Finished)
                    && navigation_allowed(&url, pdf)
                {
                    loaded_status.store(1, Ordering::Release);
                }
            })
            .with_ipc_handler(move |request| {
                if graphic && navigation_allowed(&request.uri().to_string(), false) {
                    match request.body().as_str() {
                        "tshell:ready" => render_status.store(1, Ordering::Release),
                        "tshell:error" => render_status.store(2, Ordering::Release),
                        _ => {}
                    }
                }
            })
            .with_bounds(web_bounds(bounds))
            .build_as_child(&parent)?;
        Ok(Self {
            webview,
            html,
            page,
            pdf,
            graphic,
            status,
            revision: 0,
            bounds,
            visible: false,
        })
    }

    pub fn show_html(&mut self, html: &str, bounds: Bounds<Pixels>) -> wry::Result<()> {
        if self.html != html {
            *self.page.write().unwrap_or_else(|error| error.into_inner()) = html.to_owned();
            self.revision += 1;
            self.status.store(0, Ordering::Release);
            self.webview
                .load_url(&format!("{PAGE_URL}?revision={}", self.revision))?;
            self.html = html.to_owned();
        }
        self.show_bounds(bounds)
    }

    pub fn show_pdf(&mut self, bounds: Bounds<Pixels>) -> wry::Result<()> {
        self.show_bounds(bounds)
    }

    fn show_bounds(&mut self, bounds: Bounds<Pixels>) -> wry::Result<()> {
        if self.bounds != bounds {
            self.webview.set_bounds(web_bounds(bounds))?;
            self.bounds = bounds;
        }
        if !self.visible {
            self.webview.set_visible(true)?;
            self.visible = true;
        }
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible && self.webview.set_visible(false).is_ok() {
            self.visible = false;
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }
    pub fn is_pdf(&self) -> bool {
        self.pdf
    }
    pub fn is_graphic(&self) -> bool {
        self.graphic
    }
    pub fn html(&self) -> &str {
        &self.html
    }
    pub fn status(&self) -> RenderStatus {
        match self.status.load(Ordering::Acquire) {
            1 => RenderStatus::Ready,
            2 => RenderStatus::Failed,
            _ => RenderStatus::Loading,
        }
    }
}

fn navigation_allowed(url: &str, pdf: bool) -> bool {
    let destination = url.split(['?', '#']).next().unwrap_or_default();
    destination == if pdf { PDF_URL } else { PAGE_URL }
}

fn response(
    status: StatusCode,
    mime: &'static str,
    bytes: Cow<'static, [u8]>,
) -> Response<Cow<'static, [u8]>> {
    let mut response = Response::new(bytes);
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        wry::http::HeaderValue::from_static(mime),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        wry::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        wry::http::HeaderValue::from_static("nosniff"),
    );
    response
}

fn page_response(
    request: &Request<Vec<u8>>,
    page: &RwLock<String>,
) -> Response<Cow<'static, [u8]>> {
    if request.uri().scheme_str() != Some("tshell-preview")
        || request.uri().host() != Some("localhost")
        || request.method() != "GET"
    {
        return response(StatusCode::NOT_FOUND, "text/plain", Cow::Borrowed(b""));
    }
    match request.uri().path() {
        "/document.html" => response(
            StatusCode::OK,
            "text/html; charset=utf-8",
            Cow::Owned(
                page.read()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_bytes()
                    .to_vec(),
            ),
        ),
        "/viewer.js" => response(
            StatusCode::OK,
            "text/javascript",
            Cow::Borrowed(include_bytes!("../../../../assets/preview/viewer.js")),
        ),
        "/mermaid.js" => response(
            StatusCode::OK,
            "text/javascript",
            Cow::Borrowed(include_bytes!("../../../../assets/preview/mermaid.js")),
        ),
        _ => response(StatusCode::NOT_FOUND, "text/plain", Cow::Borrowed(b"")),
    }
}

fn pdf_response(request: &Request<Vec<u8>>, bytes: &[u8]) -> Response<Cow<'static, [u8]>> {
    if request.uri().scheme_str() != Some("tshell-pdf")
        || request.uri().host() != Some("localhost")
        || request.uri().path() != "/document.pdf"
        || request.method() != "GET"
    {
        return response(StatusCode::NOT_FOUND, "text/plain", Cow::Borrowed(b""));
    }
    response(
        StatusCode::OK,
        "application/pdf",
        Cow::Owned(bytes.to_vec()),
    )
}

fn web_bounds(bounds: Bounds<Pixels>) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(
            f64::from(bounds.origin.x),
            f64::from(bounds.origin.y),
        )
        .into(),
        size: wry::dpi::LogicalSize::new(
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
        )
        .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_navigation_rejects_external_hosts_and_other_local_paths() {
        assert!(navigation_allowed(PAGE_URL, false));
        assert!(navigation_allowed(
            &format!("{PAGE_URL}?revision=2#title"),
            false
        ));
        assert!(navigation_allowed(PDF_URL, true));
        for url in [
            "https://example.com",
            "http://tshell-pdf.attacker/document.pdf",
            "http://tshell-preview.localhost/other.html",
            "file:///C:/private.txt",
            PDF_URL,
        ] {
            assert!(!navigation_allowed(url, false), "{url}");
        }
    }

    #[test]
    fn protocol_serves_only_document_and_bundled_renderer_assets() {
        let page = RwLock::new("<h1>Test</h1>".into());
        for path in ["/document.html", "/mermaid.js", "/viewer.js"] {
            let request = Request::builder()
                .uri(format!("tshell-preview://localhost{path}"))
                .body(Vec::new())
                .unwrap();
            let result = page_response(&request, &page);
            assert_eq!(result.status(), StatusCode::OK);
            assert!(!result.body().is_empty());
        }
        for url in [
            "http://tshell-preview.localhost/../../private",
            "http://attacker/document.html",
        ] {
            let request = Request::builder().uri(url).body(Vec::new()).unwrap();
            assert_eq!(
                page_response(&request, &page).status(),
                StatusCode::NOT_FOUND
            );
        }
    }
}
