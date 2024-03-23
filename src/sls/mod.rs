/* SPDX-License-Identifier: BSD-2-Clause */
pub mod ftrace;

use anyhow::Result;
use std::fmt::Debug;
use std::time::SystemTime;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;
use zbus::connection::Connection;

use crate::Service;

#[zbus::proxy(
    interface = "com.steampowered.SteamOSLogSubmitter.Manager",
    default_service = "com.steampowered.SteamOSLogSubmitter",
    default_path = "/com/steampowered/SteamOSLogSubmitter/Manager"
)]
trait Daemon {
    async fn log(
        &self,
        timestamp: f64,
        module: &str,
        level: u32,
        message: &str,
    ) -> zbus::Result<()>;
}

struct StringVisitor {
    string: String,
}

struct LogLine {
    timestamp: f64,
    module: String,
    level: u32,
    message: String,
}

pub struct LogReceiver
where
    Self: 'static,
{
    receiver: UnboundedReceiver<LogLine>,
    sender: UnboundedSender<LogLine>,
    proxy: DaemonProxy<'static>,
}

pub struct LogLayer {
    queue: UnboundedSender<LogLine>,
}

impl Visit for StringVisitor {
    fn record_debug(&mut self, _: &Field, value: &dyn Debug) {
        self.string.push_str(format!("{value:?}").as_str());
    }
}

impl LogReceiver {
    pub async fn new(connection: Connection) -> Result<LogReceiver> {
        let proxy = DaemonProxy::new(&connection).await?;
        let (sender, receiver) = unbounded_channel();
        Ok(LogReceiver {
            receiver,
            sender,
            proxy,
        })
    }
}

impl Service for LogReceiver {
    const NAME: &'static str = "SLS log receiver";

    async fn run(&mut self) -> Result<()> {
        while let Some(message) = self.receiver.recv().await {
            let _ = self
                .proxy
                .log(
                    message.timestamp,
                    message.module.as_ref(),
                    message.level,
                    message.message.as_ref(),
                )
                .await;
        }
        Ok(())
    }
}

impl LogLayer {
    pub async fn new(receiver: &LogReceiver) -> LogLayer {
        LogLayer {
            queue: receiver.sender.clone(),
        }
    }
}

impl<S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target();
        if !target.starts_with("steamos_workerd::sls") {
            // Don't forward non-SLS-related logs to SLS
            return;
        }
        let target = target
            .split("::")
            .skip(2)
            .fold(String::from("steamos_workerd"), |prefix, suffix| {
                prefix + "." + suffix
            });
        let level = match *event.metadata().level() {
            Level::TRACE => 10,
            Level::DEBUG => 10,
            Level::INFO => 20,
            Level::WARN => 30,
            Level::ERROR => 40,
        };
        let mut builder = StringVisitor {
            string: String::new(),
        };
        event.record(&mut builder);
        let text = builder.string;
        let now = SystemTime::now();
        let time = match now.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(duration) => duration.as_secs_f64(),
            Err(_) => 0.0,
        };
        let _ = self.queue.send(LogLine {
            timestamp: time,
            module: target,
            level,
            message: text,
        });
    }
}
