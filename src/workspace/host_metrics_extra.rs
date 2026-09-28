use super::super::metrics_config::Metric;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub(super) struct Sample {
    time: Option<f64>,
    counters: BTreeMap<String, Vec<u64>>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Extra {
    pub sections: Vec<(Metric, String)>,
    pub labels: BTreeMap<Metric, String>,
}

fn bytes(value: f64) -> String {
    if value >= 1073741824. {
        format!("{:.1} GiB", value / 1073741824.)
    } else if value >= 1048576. {
        format!("{:.1} MiB", value / 1048576.)
    } else if value >= 1024. {
        format!("{:.1} KiB", value / 1024.)
    } else {
        format!("{value:.0} B")
    }
}

enum CounterKind {
    Network,
    Disk,
    Cpu,
}

fn value<'a>(frame: &'a str, key: &str) -> &'a str {
    frame
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .unwrap_or("")
        .trim()
}

pub(super) fn parse(frame: &str, previous: &mut Sample) -> Extra {
    let mut extra = counters(frame, previous);
    system(frame, &mut extra);
    memory(frame, &mut extra);
    devices(frame, &mut extra);
    extra
}

fn counters(frame: &str, previous: &mut Sample) -> Extra {
    let clock = value(frame, "sample_time ");
    let time = (if clock.is_empty() {
        value(frame, "uptime ")
    } else {
        clock
    })
    .split_whitespace()
    .next()
    .and_then(|v| v.parse::<f64>().ok());
    let elapsed = time
        .zip(previous.time)
        .map(|(now, old)| now - old)
        .filter(|v| v.is_finite() && *v > 0.);
    let mut current = Sample {
        time,
        ..Sample::default()
    };
    let mut extra = Extra::default();
    let mut networks = Vec::new();
    let devices: std::collections::BTreeSet<_> = frame
        .lines()
        .filter_map(|line| line.strip_prefix("iototal "))
        .map(str::trim)
        .collect();
    let mut disks = Vec::new();
    let mut disk_rates = BTreeMap::new();
    let mut cores = Vec::new();
    for line in frame.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(key) = fields.first().copied() else {
            continue;
        };
        let kind = if key.starts_with("net:") {
            CounterKind::Network
        } else if key.starts_with("io:") {
            CounterKind::Disk
        } else if key
            .strip_prefix("cpu")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        {
            CounterKind::Cpu
        } else {
            continue;
        };
        let Ok(numbers) = fields[1..]
            .iter()
            .map(|v| v.parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        let delta = previous
            .counters
            .get(key)
            .filter(|old| old.len() == numbers.len())
            .and_then(|old| {
                numbers
                    .iter()
                    .zip(old)
                    .map(|(now, old)| now.checked_sub(*old))
                    .collect::<Option<Vec<_>>>()
            });
        let rates = delta.as_ref().zip(elapsed).map(|(delta, elapsed)| {
            delta
                .iter()
                .map(|v| *v as f64 / elapsed)
                .collect::<Vec<_>>()
        });
        match kind {
            CounterKind::Network if numbers.len() == 2 => {
                let rate = rates
                    .as_ref()
                    .map(|r| format!("↓ {}/s  ↑ {}/s", bytes(r[0]), bytes(r[1])))
                    .unwrap_or_else(|| "↓ …  ↑ …".into());
                let name = &key[4..];
                if name == value(frame, "primary ") {
                    extra.labels.insert(
                        Metric::Download,
                        rates
                            .as_ref()
                            .map(|r| format!("↓ {}/s", bytes(r[0])))
                            .unwrap_or_else(|| "↓ …".into()),
                    );
                    extra.labels.insert(
                        Metric::Upload,
                        rates
                            .as_ref()
                            .map(|r| format!("↑ {}/s", bytes(r[1])))
                            .unwrap_or_else(|| "↑ …".into()),
                    );
                }
                let friendly = value(frame, &format!("netname {name} "));
                let name = if friendly.is_empty() { name } else { friendly };
                networks.push(
                    crate::t!(
                        "metrics.interface",
                        name = name,
                        rate = rate,
                        down = bytes(numbers[0] as f64),
                        up = bytes(numbers[1] as f64)
                    )
                    .to_string(),
                );
            }
            CounterKind::Disk if numbers.len() == 4 && devices.contains(&key[3..]) => {
                if let Some(r) = &rates {
                    disk_rates.insert(&key[3..], (r[1] * 512., r[3] * 512.));
                }
                let rate = rates
                    .map(|r| {
                        crate::t!(
                            "metrics.read_write",
                            read = bytes(r[1] * 512.),
                            write = bytes(r[3] * 512.),
                            read_iops = format!("{:.0}", r[0]),
                            write_iops = format!("{:.0}", r[2])
                        )
                        .to_string()
                    })
                    .unwrap_or_else(|| crate::t!("metrics.waiting").to_string());
                disks.push(format!("{}   {rate}", &key[3..]));
            }
            CounterKind::Cpu if numbers.len() >= 4 => {
                let usage = delta
                    .and_then(|d| {
                        let total: u64 = d.iter().take(8).sum();
                        let idle = d[3] + d.get(4).copied().unwrap_or(0);
                        (total > 0 && idle <= total)
                            .then(|| format!("{:.0}%", 100. * (total - idle) as f64 / total as f64))
                    })
                    .unwrap_or_else(|| "…".into());
                cores.push(format!("{} {usage}", key));
            }
            _ => {}
        }
        current.counters.insert(key.to_owned(), numbers);
    }
    if !cores.is_empty() {
        extra.sections.push((Metric::Cpu, cores.join("\n")));
    }
    if !networks.is_empty() {
        extra.sections.push((
            Metric::Download,
            crate::t!(
                "metrics.primary_route",
                route = value(frame, "primary "),
                interfaces = networks.join("\n\n")
            )
            .to_string(),
        ));
    }
    if let Some((_, detail)) = extra
        .sections
        .iter()
        .find(|(metric, _)| *metric == Metric::Download)
    {
        extra.sections.push((Metric::Upload, detail.clone()));
    }
    if !disks.is_empty() {
        extra.sections.push((Metric::DiskIo, disks.join("\n")));
        if !devices.is_empty() {
            // Require every selected disk's delta, avoiding a partial total on hotplug/reset.
            let total = devices.iter().try_fold((0., 0.), |(read, write), name| {
                disk_rates.get(name).map(|(r, w)| (read + r, write + w))
            });
            extra.labels.insert(
                Metric::DiskIo,
                total
                    .map(|(read, write)| {
                        crate::t!(
                            "metrics.disk_io_short",
                            read = format!("{}/s", bytes(read)),
                            write = format!("{}/s", bytes(write))
                        )
                        .to_string()
                    })
                    .unwrap_or_else(|| {
                        crate::t!("metrics.disk_io_short", read = "…", write = "…").to_string()
                    }),
            );
        }
    }
    *previous = current;
    extra
}

fn system(frame: &str, extra: &mut Extra) {
    let time = value(frame, "uptime ")
        .split_whitespace()
        .next()
        .and_then(|v| v.parse::<f64>().ok());
    let mut system = Vec::new();
    if let Some(seconds) = time {
        extra.labels.insert(
            Metric::Uptime,
            crate::t!(
                "metrics.uptime_short",
                days = (seconds / 86400.) as u64,
                hours = (seconds / 3600.) as u64 % 24
            )
            .to_string(),
        );
        system.push(
            crate::t!(
                "metrics.uptime_long",
                days = (seconds / 86400.) as u64,
                hours = (seconds / 3600.) as u64 % 24,
                minutes = (seconds / 60.) as u64 % 60
            )
            .to_string(),
        );
    }
    if !value(frame, "processes ").is_empty() {
        system.push(crate::t!("metrics.processes", count = value(frame, "processes ")).to_string());
    }
    extra.sections.push((Metric::System, system.join("\n")));
    extra.sections.push((Metric::Uptime, system.join("\n")));
}

fn memory(frame: &str, extra: &mut Extra) {
    let number = |key| {
        value(frame, key)
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
    };
    let mut memory = Vec::new();
    for (label, key) in [
        (crate::t!("metrics.available").to_string(), "MemAvailable:"),
        (crate::t!("metrics.cached").to_string(), "Cached:"),
        (crate::t!("metrics.buffers").to_string(), "Buffers:"),
    ] {
        if let Some(n) = number(key) {
            memory.push(format!("{label} {}", bytes(n as f64 * 1024.)));
        }
    }
    if let Some((total, free)) = number("SwapTotal:").zip(number("SwapFree:")) {
        memory.push(
            crate::t!(
                "metrics.swap",
                used = bytes(total.saturating_sub(free) as f64 * 1024.),
                total = bytes(total as f64 * 1024.)
            )
            .to_string(),
        );
    }
    let page: Vec<_> = value(frame, "pagefile ")
        .split_whitespace()
        .filter_map(|v| v.parse::<f64>().ok())
        .collect();
    if page.len() == 2 {
        memory.push(
            crate::t!(
                "metrics.pagefile",
                used = bytes(page[0] * 1024.),
                total = bytes(page[1] * 1024.)
            )
            .to_string(),
        );
    }
    extra.sections.push((Metric::Memory, memory.join("\n")));
}

fn devices(frame: &str, extra: &mut Extra) {
    let mut seen = std::collections::BTreeSet::new();
    let mounts = frame
        .lines()
        .filter_map(|l| l.strip_prefix("mount "))
        .filter_map(|l| {
            let f: Vec<_> = l.split_whitespace().collect();
            if f.len() < 6 || !seen.insert(f[0]) {
                return None;
            }
            let total = f[1].parse::<f64>().ok()? * 1024.;
            let used = f[2].parse::<f64>().ok()? * 1024.;
            Some(format!(
                "{}  {}   {} / {} · {}",
                f[0],
                f[5..].join(" "),
                bytes(used),
                bytes(total),
                f[4]
            ))
        })
        .collect::<Vec<_>>();
    if !mounts.is_empty() {
        extra.sections.push((Metric::Disk, mounts.join("\n")));
    }
    let temperatures: Vec<_> = frame
        .lines()
        .filter_map(|line| line.strip_prefix("temp "))
        .filter_map(|line| {
            let (name, raw) = line.rsplit_once(' ')?;
            let celsius = raw.parse::<f64>().ok()? / 1000.;
            (-20. ..=150.).contains(&celsius).then_some((name, celsius))
        })
        .collect();
    let best = temperatures
        .iter()
        .filter_map(|(name, celsius)| cpu_sensor_rank(name).map(|rank| (rank, *celsius)))
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)));
    let mut descriptions = Vec::new();
    if let Some((_, celsius)) = best {
        extra.labels.insert(Metric::Temperature, {
            crate::t!("metrics.cpu_temp_short", value = format!("{celsius:.0}")).to_string()
        });
        descriptions
            .push(crate::t!("metrics.cpu_sensor", celsius = format!("{celsius:.1}")).to_string());
    }
    for (label, chips) in [
        (
            crate::t!("metrics.gpu"),
            &["amdgpu", "nouveau", "nvidia"][..],
        ),
        (crate::t!("metrics.disk_temp"), &["nvme", "drivetemp"][..]),
    ] {
        let readings: Vec<_> = temperatures
            .iter()
            .filter(|(name, _)| {
                let chip = name
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                chips.contains(&chip.as_str())
            })
            .map(|(_, celsius)| *celsius)
            .collect();
        if let Some(celsius) = readings.iter().copied().max_by(f64::total_cmp) {
            descriptions.push(format!(
                "{label}{} {celsius:.1} °C",
                if readings.len() > 1 {
                    crate::t!("metrics.peak")
                } else {
                    "".into()
                }
            ));
        }
    }
    if !descriptions.is_empty() {
        extra
            .sections
            .push((Metric::Temperature, descriptions.join("\n")));
    }
}

fn cpu_sensor_rank(name: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase();
    if name.contains("x86_pkg_temp")
        || (name.contains("coretemp") && name.contains("package"))
        || (name.contains("k10temp") && name.contains("tdie"))
    {
        Some(0)
    } else if name.contains("k10temp") && name.contains("tctl") {
        Some(1)
    } else if name.contains("cpu") || name.contains("coretemp") {
        Some(2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn total_io_counts_selected_disks_once_and_waits_on_resets() {
        let mut previous = Sample::default();
        let first = "uptime 10\niototal sda\niototal sdb\niototal sda\nio:sda 1 10 1 20\nio:sdb 1 10 1 20\nio:sda1 1 10 1 20\nio:dm-0 1 10 1 20";
        parse(first, &mut previous);
        let second = first
            .replace("uptime 10", "uptime 15")
            .replace("1 10 1 20", "2 20 3 40");
        let stats = parse(&second, &mut previous);
        // Locale-independent: other tests switch the process-global locale in parallel.
        let label = &stats.labels[&Metric::DiskIo];
        assert!(
            label.contains("2.0 KiB/s") && label.contains("4.0 KiB/s"),
            "unexpected disk I/O label {label}"
        );
        let detail = &stats
            .sections
            .iter()
            .find(|(metric, _)| *metric == Metric::DiskIo)
            .unwrap()
            .1;
        assert!(!detail.contains("sda1") && !detail.contains("dm-0"));
        let reset = second
            .replace("uptime 15", "uptime 20")
            .replace("io:sdb 2 20 3 40", "io:sdb 0 0 0 0");
        // Locale-independent: a reset yields the pending label in any locale.
        assert_eq!(
            parse(&reset, &mut previous).labels[&Metric::DiskIo]
                .matches('…')
                .count(),
            2
        );
    }
    #[test]
    fn temperature_prefers_cpu_package_and_never_substitutes_other_sensors() {
        let stats = parse(
            "temp nvme Composite 80000\ntemp coretemp Core 0 60000\ntemp coretemp Package id 0 55000\ntemp coretemp Package id 1 58000\ntemp acpitz 90000",
            &mut Sample::default(),
        );
        assert_eq!(stats.labels[&Metric::Temperature], "CPU 58 °C");
        // The disk label is translated; only the reading matters here.
        assert!(
            stats
                .sections
                .iter()
                .any(|(_, text)| text.contains("80.0 °C"))
        );
        assert!(
            !parse("temp nvme Composite 80000", &mut Sample::default())
                .labels
                .contains_key(&Metric::Temperature)
        );
    }
    #[test]
    fn disk_sectors_core_percent_and_optional_sensors() {
        let mut previous = Sample::default();
        parse(
            "uptime 20\niototal sda\nio:sda 10 100 20 200\ncpu0 10 0 0 90 0 0 0 0 0 0",
            &mut previous,
        );
        let stats = parse(
            "uptime 25\niototal sda\nio:sda 20 110 40 220\ncpu0 20 0 0 100 0 0 0 0 0 0\ntemp package 42500\ntemp broken nope\nSwapTotal: 1024 kB\nSwapFree: 512 kB",
            &mut previous,
        );
        let text = stats
            .sections
            .iter()
            .map(|(_, body)| body.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        // Values are locale-independent; only the surrounding labels are translated.
        assert!(text.contains("1.0 KiB") && text.contains("2.0 KiB"));
        assert!(text.contains('2') && text.contains('4'));
        assert!(text.contains("cpu0 50%"));
        assert!(!text.contains("package"));
        assert!(text.contains("512.0 KiB") && text.contains("1.0 MiB"));
        assert!(!text.contains("broken"));
    }
    #[test]
    fn rates_use_remote_elapsed_time_and_reset_without_spikes() {
        let mut previous = Sample::default();
        let first = parse("uptime 10\nprimary eth0\nnet:eth0 100 200\n", &mut previous);
        assert_eq!(
            first.labels.get(&Metric::Download).map(String::as_str),
            Some("↓ …")
        );
        let second = parse(
            "uptime 12\nprimary eth0\nnet:eth0 2148 4296\nio:sda 10 20 30 40\n",
            &mut previous,
        );
        assert_eq!(
            second.labels.get(&Metric::Download).map(String::as_str),
            Some("↓ 1.0 KiB/s")
        );
        assert_eq!(
            second.labels.get(&Metric::Upload).map(String::as_str),
            Some("↑ 2.0 KiB/s")
        );
        let reset = parse("uptime 13\nprimary eth0\nnet:eth0 1 2\n", &mut previous);
        assert_eq!(
            reset.labels.get(&Metric::Download).map(String::as_str),
            Some("↓ …")
        );
        let missing = parse("uptime 14\n", &mut previous);
        assert!(!missing.labels.contains_key(&Metric::Download));
    }
}
