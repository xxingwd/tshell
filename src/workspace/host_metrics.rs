#[path = "host_metrics_extra.rs"]
mod extra;
#[cfg(test)]
use super::metrics_config::Config;
use super::metrics_config::Metric;
use crate::tmux_client::HostConfig;
use anyhow::{Context, Result, bail, ensure};
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    time::timeout,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Local,
    Remote(HostConfig),
}

#[derive(Clone, Debug)]
pub(super) struct Stats {
    cpu: Option<f64>,
    memory: Option<(u64, u64)>,
    disk: Option<(u64, u64, String)>,
    system: String,
    release: String,
    model: String,
    cores: String,
    extra: extra::Extra,
}
impl Stats {
    #[cfg(test)]
    pub fn items(&self, config: &Config) -> Vec<(String, String)> {
        config
            .resources()
            .filter_map(|metric| self.item(metric))
            .collect()
    }
    pub fn item(&self, metric: Metric) -> Option<(String, String)> {
        let label = match metric {
            Metric::Cpu => Some(
                self.cpu
                    .map(|v| crate::t!("metrics.cpu_short", value = format!("{v:.0}%")).to_string())
                    .unwrap_or_else(|| crate::t!("metrics.cpu_short", value = "…").to_string()),
            ),
            Metric::Memory => self
                .memory
                .map(|(used, total)| format!("{:.1}/{:.1} GiB", gib(used), gib(total))),
            Metric::Disk => self.disk.as_ref().map(|(_, _, percent)| {
                crate::t!("metrics.disk_usage", percent = percent).to_string()
            }),
            Metric::System => Some(self.release.clone()),
            Metric::Geometry | Metric::SshRtt | Metric::TmuxRtt | Metric::Unknown => None,
            _ => self.extra.labels.get(&metric).cloned(),
        }?;
        let details = self
            .extra
            .sections
            .iter()
            .filter(|(kind, _)| *kind == metric)
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let detail = match metric {
            Metric::System => crate::t!(
                "metrics.system_detail",
                system = self.system,
                model = self.model,
                cores = self.cores,
                details = details
            )
            .to_string(),
            Metric::Memory => format!("{}\n{}", label, details),
            _ if details.is_empty() => label.clone(),
            _ => details,
        };
        Some((label, detail))
    }
}

fn gib(kib: u64) -> f64 {
    kib as f64 / 1048576.
}
#[derive(Clone, Copy, Debug, Default)]
struct Cpu {
    total: u64,
    idle: u64,
}
fn cpu_usage(previous: Cpu, current: Cpu) -> Option<f64> {
    let total = current.total.checked_sub(previous.total)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    (total > 0 && idle <= total).then(|| 100. * (total - idle) as f64 / total as f64)
}
#[derive(Default)]
struct Previous {
    cpu: Option<Cpu>,
    extra: extra::Sample,
}
fn parse(frame: &str, previous: &mut Previous) -> Result<Stats> {
    let value = |key: &str| {
        frame
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap_or("")
            .trim()
    };
    let counters = value("cpu ")
        .split_whitespace()
        .take(8)
        .map(str::parse::<u64>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    ensure!(
        counters.is_empty() || counters.len() >= 4,
        "{}",
        crate::t!("metrics.cpu_count_failed")
    );
    let current = (counters.len() >= 4).then(|| Cpu {
        total: counters.iter().sum(),
        idle: counters[3] + counters.get(4).copied().unwrap_or(0),
    });
    let cpu = previous
        .cpu
        .zip(current)
        .and_then(|(previous, current)| cpu_usage(previous, current));
    previous.cpu = current;
    let number = |key| {
        value(key)
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
    };
    let memory = number("MemTotal:")
        .zip(number("MemAvailable:"))
        .and_then(|(total, available)| {
            total
                .checked_sub(available)
                .filter(|_| total > 0)
                .map(|used| (used, total))
        });
    let disk_fields: Vec<_> = value("disk ").split_whitespace().collect();
    let disk = (|| {
        let total = disk_fields.get(1)?.parse::<u64>().ok()?;
        let used = disk_fields.get(2)?.parse::<u64>().ok()?;
        let percent = *disk_fields.get(4)?;
        percent.strip_suffix('%')?.parse::<u32>().ok()?;
        Some((used, total, percent.to_owned()))
    })();
    Ok(Stats {
        extra: extra::parse(frame, &mut previous.extra),
        cpu,
        memory,
        disk,
        release: if value("release ").is_empty() {
            value("os ")
        } else {
            value("release ")
        }
        .to_owned(),
        system: format!("{} · {}", value("os "), value("arch ")),
        model: value("model ").to_owned(),
        cores: value("cores ").to_owned(),
    })
}

pub(super) struct Monitor {
    _cancel: async_channel::Sender<()>,
}
impl Monitor {
    pub fn start(
        target: Target,
        metrics: std::collections::BTreeSet<Metric>,
    ) -> (
        Self,
        async_channel::Receiver<std::result::Result<Stats, String>>,
    ) {
        let (cancel, cancelled) = async_channel::bounded::<()>(1);
        let (sender, receiver) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = sender.send_blocking(Err(error.to_string()));
                    return;
                }
            };
            runtime.block_on(async {
                let mut platform = None;
                let mut failures = 0;
                loop {
                    let result = tokio::select! {
                        _ = cancelled.recv() => break,
                        result = collect(&target, &metrics, &mut platform, &mut failures, &sender) => result,
                    };
                    let error = result
                        .err()
                        .map(|error| format!("{error:#}"))
                        .unwrap_or_else(|| crate::t!("metrics.disconnected").to_string());
                    failures += 1;
                    let stopped = failures >= 3;
                    let error = if stopped {
                        crate::t!("metrics.stopped_after_failures", error = error).to_string()
                    } else {
                        error
                    };
                    tokio::select! { _ = cancelled.recv() => break, _ = sender.send(Err(error)) => {} }
                    if stopped { break; }
                    tokio::select! { _ = cancelled.recv() => break, _ = tokio::time::sleep(Duration::from_secs(30)) => {} }
                }
            });
        });
        (Self { _cancel: cancel }, receiver)
    }
}
async fn collect(
    target: &Target,
    metrics: &std::collections::BTreeSet<Metric>,
    platform: &mut Option<super::metrics_transport::Platform>,
    failures: &mut u8,
    sender: &async_channel::Sender<std::result::Result<Stats, String>>,
) -> Result<()> {
    let detected = match *platform {
        Some(platform) => platform,
        None => super::metrics_transport::detect(target).await?,
    };
    let mut connection = super::metrics_transport::connect(target, detected, metrics).await?;
    let mut lines = BufReader::new(&mut connection.reader).lines();
    let mut frame = String::new();
    let mut previous = Previous::default();
    let result = async {
        loop {
            let line = timeout(Duration::from_secs(45), lines.next_line())
                .await
                .context(crate::t!("metrics.timeout"))??;
            let Some(line) = line else {
                bail!("{}", crate::t!("metrics.closed"));
            };
            match line.as_str() {
                "TSHELL_UNSUPPORTED" => bail!("{}", crate::t!("metrics.linux_only")),
                "TSHELL_METRICS_BEGIN" => frame.clear(),
                "TSHELL_METRICS_END" => {
                    sender.send(Ok(parse(&frame, &mut previous)?)).await?;
                    *platform = Some(detected);
                    *failures = 0;
                }
                _ => {
                    ensure!(
                        frame.len() + line.len() < 1048576,
                        "{}",
                        crate::t!("metrics.response_too_large")
                    );
                    frame.push_str(&line);
                    frame.push('\n');
                }
            }
        }
    }
    .await;
    result.context(crate::t!("metrics.stopped"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "reads selected metrics from local Windows or TSHELL_SSH_TEST_HOST"]
    fn selected_metrics_collect_only_memory() -> Result<()> {
        let target = match std::env::var("TSHELL_SSH_TEST_HOST") {
            Ok(destination) => Target::Remote(HostConfig {
                destination,
                name: String::new(),
                user: String::new(),
                port: None,
                identity_file: None,
                tmux: false,
                socket: None,
            }),
            Err(_) => Target::Local,
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let platform = super::super::metrics_transport::detect(&target).await?;
                let mut connection = super::super::metrics_transport::connect(
                    &target,
                    platform,
                    &[Metric::Memory].into_iter().collect(),
                )
                .await?;
                let mut lines = BufReader::new(&mut connection.reader).lines();
                let frame = timeout(Duration::from_secs(60), async {
                    let mut frame = String::new();
                    while let Some(line) = lines.next_line().await? {
                        if line == "TSHELL_METRICS_END" {
                            return Ok::<_, anyhow::Error>(frame);
                        }
                        frame.push_str(&line);
                        frame.push('\n');
                    }
                    bail!("collector closed before first sample")
                })
                .await??;
                assert!(!frame.lines().any(|line| {
                    ["cpu ", "net:", "io:", "disk ", "mount ", "temp "]
                        .iter()
                        .any(|prefix| line.starts_with(prefix))
                }));
                let stats = parse(&frame, &mut Previous::default())?;
                assert!(stats.memory.is_some());
                assert!(stats.cpu.is_none());
                assert!(stats.disk.is_none());
                Ok(())
            })
    }

    #[test]
    fn concise_labels_and_metric_specific_details() {
        let stats = parse("cpu 10 0 0 90\ncpu0 10 0 0 90\nrelease Ubuntu 24.04 LTS\nos Linux 6.8\nmodel Test CPU\ncores 1\nMemTotal: 1048576\nMemAvailable: 524288\nmount /dev/sda1 1000 400 600 40% /\ndisk /dev/sda1 1000 400 600 40% /", &mut Previous::default()).unwrap();
        let config = Config(
            vec![Metric::System, Metric::Memory, Metric::Cpu, Metric::Disk]
                .into_iter()
                .map(|metric| super::super::metrics_config::Item {
                    metric,
                    enabled: true,
                    side: None,
                })
                .collect(),
        );
        let items = stats.items(&config);
        assert_eq!(items[0].0, "Ubuntu 24.04 LTS");
        assert!(items[0].1.contains("Linux 6.8") && items[0].1.contains("Test CPU"));
        assert_eq!(items[1].0, "0.5/1.0 GiB");
        assert!(!items[1].1.contains("Linux") && !items[1].1.contains("计算"));
        assert_eq!(items[2].1, "cpu0 …");
        assert!(items[3].1.starts_with("/dev/sda1") && !items[3].1.contains("逻辑核"));
    }
    #[test]
    fn footer_obeys_order_and_hides_disabled_or_unavailable_metrics() {
        let stats = parse(
            "cpu 10 0 0 90\nMemTotal: 2048\nMemAvailable: 1024",
            &mut Previous::default(),
        )
        .unwrap();
        let config = Config(vec![
            super::super::metrics_config::Item {
                metric: Metric::Memory,
                enabled: true,
                side: None,
            },
            super::super::metrics_config::Item {
                metric: Metric::Temperature,
                enabled: true,
                side: None,
            },
            super::super::metrics_config::Item {
                metric: Metric::Cpu,
                enabled: true,
                side: None,
            },
            super::super::metrics_config::Item {
                metric: Metric::System,
                enabled: false,
                side: None,
            },
        ]);
        let items = stats.items(&config);
        assert_eq!(items.len(), 2);
        assert!(items[0].0.ends_with("GiB"));
        assert!(items[1].0.starts_with("CPU"));
        assert!(stats.items(&Config(vec![])).is_empty());
    }
    #[test]
    fn cpu_excludes_guest_and_counts_iowait_as_idle() {
        let mut previous = Previous::default();
        let first = "cpu 100 0 50 800 50 0 0 0 20 0\nMemTotal: 16384 kB\nMemAvailable: 4096 kB\ndisk /dev/root 1000 400 600 40% /\n";
        let stats = parse(first, &mut previous).unwrap();
        assert!(stats.cpu.is_none());
        assert_eq!(stats.memory, Some((12288, 16384)));
        assert_eq!(stats.disk.unwrap().2, "40%");
        let stats = parse("cpu 120 0 60 860 60 0 0 0 25 0", &mut previous).unwrap();
        assert_eq!(stats.cpu, Some(30.));
        assert!(
            cpu_usage(
                Cpu {
                    total: 20,
                    idle: 10
                },
                Cpu { total: 10, idle: 5 }
            )
            .is_none()
        );
    }
    #[test]
    #[cfg(windows)]
    #[ignore = "runs the local Windows PowerShell/CIM collector"]
    fn real_windows_metrics() {
        let (monitor, receiver) = Monitor::start(Target::Local, Metric::ALL.into_iter().collect());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let first = timeout(Duration::from_secs(60), receiver.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(first.memory.unwrap().1 > 0);
            assert!(first.system.contains("Windows"));
            let second = timeout(Duration::from_secs(45), receiver.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!((0. ..=100.).contains(&second.cpu.unwrap()));
            assert!(second.disk.unwrap().1 > 0);
            assert!(second.extra.labels.contains_key(&Metric::Download));
            assert!(
                second
                    .extra
                    .labels
                    .get(&Metric::DiskIo)
                    .is_some_and(|label| label.contains("/s"))
            );

            drop(monitor);
            assert!(
                timeout(Duration::from_secs(3), receiver.recv())
                    .await
                    .unwrap()
                    .is_err()
            );
        });
    }
    #[test]
    #[ignore = "requires TSHELL_SSH_TEST_HOST; read-only Linux resource query"]
    fn real_remote_metrics() {
        let host = HostConfig {
            destination: std::env::var("TSHELL_SSH_TEST_HOST").unwrap(),
            name: String::new(),
            user: String::new(),
            port: None,
            identity_file: None,
            tmux: false,

            socket: None,
        };
        let (monitor, receiver) =
            Monitor::start(Target::Remote(host), Metric::ALL.into_iter().collect());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let first = timeout(Duration::from_secs(20), receiver.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(first.memory.unwrap().1 > 0);
            assert!(first.disk.unwrap().1 > 0);
            let second = timeout(Duration::from_secs(10), receiver.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!((0. ..=100.).contains(&second.cpu.unwrap()));
            assert!(second.extra.labels.contains_key(&Metric::Download));
            assert!(
                second
                    .extra
                    .labels
                    .get(&Metric::DiskIo)
                    .is_some_and(|label| label.contains("/s"))
            );
            assert!(
                second
                    .extra
                    .sections
                    .iter()
                    .any(|(title, body)| *title == Metric::Download && body.contains("/s"))
            );
            drop(monitor);
            assert!(
                timeout(Duration::from_secs(3), receiver.recv())
                    .await
                    .unwrap()
                    .is_err()
            );
        });
    }
}
