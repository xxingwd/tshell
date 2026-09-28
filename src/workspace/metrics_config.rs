use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Metric {
    Cpu,
    Memory,
    Download,
    Upload,
    Disk,
    DiskIo,
    Uptime,
    Temperature,
    System,
    Geometry,
    SshRtt,
    TmuxRtt,
    #[serde(other)]
    Unknown,
}
impl Metric {
    pub const ALL: [Self; 12] = [
        Self::System,
        Self::Cpu,
        Self::Temperature,
        Self::Memory,
        Self::Disk,
        Self::DiskIo,
        Self::Download,
        Self::Upload,
        Self::Uptime,
        Self::Geometry,
        Self::SshRtt,
        Self::TmuxRtt,
    ];
    pub fn is_resource(self) -> bool {
        match self {
            Self::Cpu
            | Self::Memory
            | Self::Download
            | Self::Upload
            | Self::Disk
            | Self::DiskIo
            | Self::Uptime
            | Self::Temperature
            | Self::System => true,
            Self::Geometry | Self::SshRtt | Self::TmuxRtt | Self::Unknown => false,
        }
    }
    pub fn default_side(self) -> Side {
        if self.is_resource() {
            Side::Left
        } else {
            Side::Right
        }
    }
    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Cpu => crate::t!("metrics.cpu"),
            Self::Memory => crate::t!("metrics.memory"),
            Self::Download => crate::t!("metrics.download"),
            Self::Upload => crate::t!("metrics.upload"),
            Self::Disk => crate::t!("metrics.disk"),
            Self::DiskIo => crate::t!("metrics.disk_io"),
            Self::Uptime => crate::t!("metrics.uptime"),
            Self::Temperature => crate::t!("metrics.temperature"),
            Self::System => crate::t!("metrics.system_info"),
            Self::Geometry => crate::t!("metrics.geometry"),
            Self::SshRtt => crate::t!("ws.diagnostics_ssh"),
            Self::TmuxRtt => crate::t!("ws.diagnostics_tmux"),
            Self::Unknown => crate::t!("metrics.unknown"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Side {
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Item {
    pub metric: Metric,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
}
impl Item {
    pub fn side(&self) -> Side {
        self.side.unwrap_or_else(|| self.metric.default_side())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct Config(pub Vec<Item>);
impl Default for Config {
    fn default() -> Self {
        Self(
            Metric::ALL
                .into_iter()
                .map(|metric| Item {
                    metric,
                    enabled: !matches!(
                        metric,
                        Metric::DiskIo | Metric::Uptime | Metric::SshRtt | Metric::TmuxRtt
                    ),
                    side: Some(metric.default_side()),
                })
                .collect(),
        )
    }
}
impl Config {
    pub fn move_to(&mut self, source: Metric, target: Metric) -> bool {
        let from = self.0.iter().position(|item| item.metric == source);
        let to = self.0.iter().position(|item| item.metric == target);
        let (Some(from), Some(to)) = (from, to) else {
            return false;
        };
        if from == to {
            return false;
        }
        let side = self.0[to].side();
        let mut item = self.0.remove(from);
        item.side = Some(side);
        self.0.insert(to, item);
        true
    }

    pub fn move_to_side(&mut self, source: Metric, side: Side) -> bool {
        let Some(from) = self.0.iter().position(|item| item.metric == source) else {
            return false;
        };
        let mut item = self.0.remove(from);
        let old_side = item.side();
        let to = match side {
            Side::Left => self
                .0
                .iter()
                .position(|item| item.side() == Side::Right)
                .unwrap_or(self.0.len()),
            Side::Right => self.0.len(),
        };
        let changed = from != to || old_side != side;
        if changed {
            item.side = Some(side);
        }
        self.0.insert(to, item);
        changed
    }

    pub fn normalized(mut self) -> Self {
        let mut seen = std::collections::BTreeSet::new();
        self.0
            .retain(|item| item.metric != Metric::Unknown && seen.insert(item.metric));
        self.0.extend(
            Metric::ALL
                .into_iter()
                .filter(|metric| !seen.contains(metric))
                .map(|metric| Item {
                    metric,
                    enabled: metric == Metric::Geometry,
                    side: Some(metric.default_side()),
                }),
        );
        self.0.sort_by_key(|item| item.side() == Side::Right);
        self
    }
    pub fn enabled(&self) -> impl Iterator<Item = Metric> + '_ {
        self.0
            .iter()
            .filter(|item| item.enabled)
            .map(|item| item.metric)
    }
    pub fn resources(&self) -> impl Iterator<Item = Metric> + '_ {
        self.enabled().filter(|metric| metric.is_resource())
    }
    pub fn probes(&self) -> (bool, bool) {
        let enabled = |metric| {
            self.0
                .iter()
                .any(|item| item.metric == metric && item.enabled)
        };
        (enabled(Metric::SshRtt), enabled(Metric::TmuxRtt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_order_disabled_choices_and_unknown_future_metrics() {
        let config: Config = serde_json::from_str(r#"[{"metric":"upload","enabled":true},{"metric":"cpu","enabled":false},{"metric":"upload","enabled":true},{"metric":"future","enabled":true},{"metric":"load","enabled":true},{"metric":"swap","enabled":true},{"metric":"pagefile","enabled":true}]"#).unwrap();
        let config = config.normalized();
        assert_eq!(config.0.len(), Metric::ALL.len());
        assert_eq!(
            config.enabled().collect::<Vec<_>>(),
            [Metric::Upload, Metric::Geometry]
        );
        assert_eq!(
            serde_json::from_str::<Config>(&serde_json::to_string(&config).unwrap()).unwrap(),
            config
        );
    }
    #[test]
    fn old_order_adds_geometry_without_enabling_network_probes() {
        let config: Config = serde_json::from_str(
            r#"[{"metric":"memory","enabled":true},{"metric":"system","enabled":false}]"#,
        )
        .unwrap();
        let config = config.normalized();
        assert_eq!(
            config.enabled().collect::<Vec<_>>(),
            [Metric::Memory, Metric::Geometry]
        );
        assert_eq!(config.resources().collect::<Vec<_>>(), [Metric::Memory]);
        assert_eq!(config.0[0].side(), Side::Left);
        assert_eq!(
            config
                .0
                .iter()
                .find(|item| item.metric == Metric::Geometry)
                .unwrap()
                .side(),
            Side::Right
        );
        assert_eq!(config.probes(), (false, false));
        assert_eq!(Config::default().probes(), (false, false));
    }
    #[test]
    fn cross_group_drop_changes_side_and_preserves_order() {
        let mut config = Config::default();
        assert!(config.move_to(Metric::Geometry, Metric::Cpu));
        assert_eq!(config.0[1].metric, Metric::Geometry);
        assert_eq!(config.0[1].side(), Side::Left);
        assert!(config.move_to_side(Metric::Cpu, Side::Right));
        assert_eq!(config.0.last().unwrap().metric, Metric::Cpu);
        assert_eq!(config.0.last().unwrap().side(), Side::Right);
        assert_eq!(
            serde_json::from_str::<Config>(&serde_json::to_string(&config).unwrap()).unwrap(),
            config
        );
    }
    #[test]
    fn empty_group_accepts_drop() {
        let mut config = Config(vec![Item {
            metric: Metric::Geometry,
            enabled: true,
            side: None,
        }]);
        assert!(config.move_to_side(Metric::Geometry, Side::Left));
        assert_eq!(config.0[0].side(), Side::Left);
        assert!(!config.move_to_side(Metric::Geometry, Side::Left));
    }
}
