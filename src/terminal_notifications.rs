//! Native notifications from live terminal output, never from screen captures.
use alacritty_terminal::vte::{Parser, Perform};
use std::sync::{
    OnceLock,
    mpsc::{self, SyncSender},
};

#[derive(Debug, PartialEq, Eq)]
struct Notice {
    title: String,
    body: String,
}

impl Notice {
    fn from_osc(params: &[&[u8]]) -> Option<Self> {
        let (title, body) = match params {
            // OSC 9;4 is the progress protocol, not a notification.
            [b"9", b"4", ..] => return None,
            [b"9", body @ ..] if !body.is_empty() => ("TShell".into(), body.join(&b';')),
            [b"777", b"notify", title, body @ ..] if !body.is_empty() => (
                String::from_utf8_lossy(title).into_owned(),
                body.join(&b';'),
            ),
            _ => return None,
        };
        let body = String::from_utf8_lossy(&body).into_owned();
        if title.trim().is_empty() && body.trim().is_empty()
            || body.len() > 8192
            || title.len() > 1024
        {
            return None;
        }
        if params.first() == Some(&b"9".as_slice()) && body.trim().is_empty() {
            return None;
        }
        Some(Self { title, body })
    }

    fn send(self) {
        static SENDER: OnceLock<Option<SyncSender<Notice>>> = OnceLock::new();
        let sender = SENDER.get_or_init(|| {
            let (tx, rx) = mpsc::sync_channel::<Notice>(32);
            match std::thread::Builder::new()
                .name("notifications".into())
                .spawn(move || {
                    for notice in rx {
                        if let Err(error) = notice.show() {
                            tracing::warn!(%error, "Could not show terminal notification");
                        }
                    }
                }) {
                Ok(_) => Some(tx),
                Err(error) => {
                    tracing::warn!(%error, "Could not start notification worker");
                    None
                }
            }
        });
        if let Some(sender) = sender {
            // Never block PTY/SSH reads on a slow desktop notification service.
            if let Err(error) = sender.try_send(self) {
                tracing::debug!(%error, "Terminal notification dropped");
            }
        }
    }

    fn show(self) -> anyhow::Result<()> {
        let mut notification = notify_rust::Notification::new();
        notification.appname("TShell").summary(&self.title);
        #[cfg(windows)]
        {
            crate::app_identity::register()?;
            notification.app_id(crate::app_identity::APP_ID);
        }
        // Freedesktop bodies interpret markup; terminal text is literal.
        #[cfg(all(unix, not(target_os = "macos")))]
        let body = self
            .body
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        #[cfg(any(windows, target_os = "macos"))]
        let body = self.body;
        notification.body(&body).show()?;
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Notifications(Parser);

impl Notifications {
    pub fn advance(&mut self, bytes: &[u8]) -> u64 {
        self.parse(bytes, Notice::send)
    }

    fn parse(&mut self, bytes: &[u8], mut emit: impl FnMut(Notice)) -> u64 {
        struct Sink<F>(F);
        impl<F: FnMut(Notice)> Perform for Sink<F> {
            fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
                if let Some(notice) = Notice::from_osc(params) {
                    (self.0)(notice);
                }
            }
        }
        let mut count = 0;
        let mut sink = Sink(|notice| {
            count += 1;
            emit(notice);
        });
        for part in bytes.split_inclusive(|b| matches!(b, 0x18 | 0x1a)) {
            if matches!(part.last(), Some(0x18 | 0x1a)) {
                self.0.advance(&mut sink, &part[..part.len() - 1]);
                // VTE dispatches OSC on CAN/SUB; cancelled notifications must
                // not escape to the desktop.
                self.0 = Parser::default();
            } else {
                self.0.advance(&mut sink, part);
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_count_only_advances_for_completed_notifications() {
        let mut parser = Notifications::default();
        assert_eq!(parser.parse(b"plain output\x1b]9;4;1;50\x07", |_| {}), 0);
        assert_eq!(parser.parse(b"\x1b]9;Ready", |_| {}), 0);
        assert_eq!(
            parser.parse(b"\x07\x1b]777;notify;Build;Done\x1b\\", |_| {}),
            2
        );
        assert_eq!(parser.parse(b"\x1b]9;cancelled\x18", |_| {}), 0);
    }

    #[test]
    #[ignore = "shows a real desktop notification; run manually in a desktop session"]
    fn notifications_native_smoke() {
        let mut parser = Notifications::default();
        let mut notices = Vec::new();
        parser.parse(
            b"\x1b]777;notify;TShell;OSC system notifications are connected.\x07",
            |n| notices.push(n),
        );
        assert_eq!(notices.len(), 1);
        notices.pop().unwrap().show().unwrap();
    }

    #[test]
    fn notifications_match_documented_osc_bytes_at_every_split() {
        let bytes =
            "text\x1b]9;任务完成;ok\x07\x1b]777;notify;Build;All tests passed\x1b\\".as_bytes();
        for split in 0..=bytes.len() {
            let mut parser = Notifications::default();
            let mut notices = Vec::new();
            parser.parse(&bytes[..split], |n| notices.push(n));
            parser.parse(&bytes[split..], |n| notices.push(n));
            assert_eq!(
                notices,
                vec![
                    Notice {
                        title: "TShell".into(),
                        body: "任务完成;ok".into()
                    },
                    Notice {
                        title: "Build".into(),
                        body: "All tests passed".into()
                    },
                ],
                "split {split}"
            );
        }
    }

    #[test]
    fn notifications_ignore_progress_empty_unrelated_and_cancelled_sequences() {
        let mut parser = Notifications::default();
        let mut notices = Vec::new();
        parser.parse(b"\x07\x1b]9;4;1;50\x07\x1b]9; \x07\x1b]2;title\x07\x1b]777;other;x;y\x07\x1b]777;notify;missing-body\x07\x1b]9;cancelled\x18\x1b]9;done\x1b\\", |n| notices.push(n));
        assert_eq!(
            notices,
            vec![Notice {
                title: "TShell".into(),
                body: "done".into()
            }]
        );
    }

    #[test]
    fn notifications_keep_pane_streams_independent() {
        let mut first = Notifications::default();
        let mut second = Notifications::default();
        let mut notices = Vec::new();
        first.parse(b"\x1b]9;first", |n| notices.push(n));
        second.parse(b"\x1b]9;second\x07", |n| notices.push(n));
        first.parse(b"\x07", |n| notices.push(n));
        assert_eq!(
            notices.iter().map(|n| n.body.as_str()).collect::<Vec<_>>(),
            ["second", "first"]
        );
    }
}
