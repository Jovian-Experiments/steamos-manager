/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, Result};
use config::builder::AsyncState;
use config::{ConfigBuilder, FileFormat, FileStoredFormat};
use std::io::ErrorKind;
use tokio::fs::{create_dir_all, read_to_string, write};
use tracing::{error, info};

use crate::daemon::DaemonContext;
use crate::{read_config_directory, AsyncFileSource};

pub(in crate::daemon) async fn read_state<C: DaemonContext>(context: &C) -> Result<C::State> {
    let path = context.state_path()?;
    let state = match read_to_string(path).await {
        Ok(state) => state,
        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                info!("No state file found, reloading default state");
                return Ok(C::State::default());
            }
            error!("Error loading state: {e}");
            return Err(e.into());
        }
    };
    Ok(toml::from_str(state.as_str())?)
}

pub(in crate::daemon) async fn write_state<C: DaemonContext>(context: &C) -> Result<()> {
    let path = context.state_path()?;
    create_dir_all(path.parent().ok_or(anyhow!(
        "Context path {} has no parent dir",
        path.to_string_lossy()
    ))?)
    .await?;
    let state = toml::to_string_pretty(&context.state())?;
    Ok(write(path, state.as_bytes()).await?)
}

pub(in crate::daemon) async fn read_config<C: DaemonContext>(context: &C) -> Result<C::Config> {
    let builder = ConfigBuilder::<AsyncState>::default();
    let system_config_path = context.system_config_path()?;
    let user_config_path = context.user_config_path()?;

    let builder = builder.add_async_source(AsyncFileSource::from(
        system_config_path.join("config.toml"),
        FileFormat::Toml,
    ));
    let builder = read_config_directory(
        builder,
        system_config_path.join("config.toml.d"),
        FileFormat::Toml.file_extensions(),
        FileFormat::Toml,
    )
    .await?;

    let builder = builder.add_async_source(AsyncFileSource::from(
        user_config_path.join("config.toml"),
        FileFormat::Toml,
    ));
    let builder = read_config_directory(
        builder,
        user_config_path.join("config.toml.d"),
        FileFormat::Toml.file_extensions(),
        FileFormat::Toml,
    )
    .await?;
    let config = builder.build().await?;
    Ok(config.try_deserialize()?)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::daemon::Daemon;
    use crate::{path, testing, write_synced};

    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Deserialize, Serialize, Copy, Clone, Default, PartialEq, Debug)]
    struct TestSubstate {
        subvalue: i32,
    }

    #[derive(Deserialize, Serialize, Copy, Clone, Default, PartialEq, Debug)]
    #[serde(default)]
    struct TestState {
        substate: TestSubstate,
        value: i32,
    }

    #[derive(Default)]
    struct TestContext {
        state: TestState,
        config: TestState,
    }

    impl DaemonContext for TestContext {
        type State = TestState;
        type Config = TestState;
        type Command = ();

        fn user_config_path(&self) -> Result<PathBuf> {
            Ok(path("user"))
        }

        fn system_config_path(&self) -> Result<PathBuf> {
            Ok(path("system"))
        }

        fn state(&self) -> TestState {
            self.state
        }

        async fn start(
            &mut self,
            state: Self::State,
            config: Self::Config,
            _daemon: &mut Daemon<Self>,
        ) -> Result<()> {
            self.state = state;
            self.config = config;
            Ok(())
        }

        async fn reload(&mut self, config: Self::Config, _daemon: &mut Daemon<Self>) -> Result<()> {
            self.config = config;
            Ok(())
        }

        async fn handle_command(
            &mut self,
            _cmd: Self::Command,
            _daemon: &mut Daemon<Self>,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_read_state() {
        let _h = testing::start();

        let context = TestContext::default();
        let state = read_state(&context).await.expect("read_state");

        assert_eq!(state, TestState::default());

        let state_path = context.state_path().expect("state_path");
        create_dir_all(state_path.parent().unwrap())
            .await
            .expect("create_dir_all");

        write_synced(
            state_path,
            "value = 1\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        let state = read_state(&context).await.expect("read_state");
        assert_eq!(
            state,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 2 }
            }
        );
    }

    #[tokio::test]
    async fn test_read_extra_state() {
        let _h = testing::start();

        let context = TestContext::default();
        let state_path = context.state_path().expect("state_path");
        create_dir_all(state_path.parent().unwrap())
            .await
            .expect("create_dir_all");

        write_synced(
            state_path,
            "value = 1\nvalue2 = 3\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        let state = read_state(&context).await.expect("read_state");
        assert_eq!(
            state,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 2 }
            }
        );
    }

    #[tokio::test]
    async fn test_read_missing_state() {
        let _h = testing::start();

        let context = TestContext::default();
        let state_path = context.state_path().expect("state_path");
        create_dir_all(state_path.parent().unwrap())
            .await
            .expect("create_dir_all");

        write_synced(state_path, "[substate]\nsubvalue = 2\n".as_bytes())
            .await
            .expect("write");

        let state = read_state(&context).await.expect("read_state");
        assert_eq!(
            state,
            TestState {
                value: 0,
                substate: TestSubstate { subvalue: 2 }
            }
        );
    }

    #[tokio::test]
    async fn test_write_state() {
        let _h = testing::start();

        let mut context = TestContext::default();
        let state_path = context.state_path().expect("state_path");

        write_state(&context).await.expect("write_state");
        let config = read_to_string(&state_path).await.expect("read_to_string");
        assert_eq!(config, "value = 0\n\n[substate]\nsubvalue = 0\n");

        context.state.value = 1;
        write_state(&context).await.expect("write_state");
        let config = read_to_string(&state_path).await.expect("read_to_string");
        assert_eq!(config, "value = 1\n\n[substate]\nsubvalue = 0\n");
    }

    #[tokio::test]
    async fn test_read_system_config() {
        let _h = testing::start();

        let context = TestContext::default();
        let config = read_config(&context).await.expect("read_config");
        assert_eq!(config, TestState::default());

        let system_config_path = context.system_config_path().expect("system_config_path");
        create_dir_all(&system_config_path)
            .await
            .expect("create_dir_all");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(config, TestState::default());

        write_synced(
            system_config_path.join("config.toml"),
            "value = 1\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 2 }
            }
        );
    }

    #[tokio::test]
    async fn test_read_user_config() {
        let _h = testing::start();

        let context = TestContext::default();
        let config = read_config(&context).await.expect("read_config");
        assert_eq!(config, TestState::default());

        let user_config_path = context.user_config_path().expect("user_config_path");
        create_dir_all(&user_config_path)
            .await
            .expect("create_dir_all");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(config, TestState::default());

        write_synced(
            user_config_path.join("config.toml"),
            "value = 1\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 2 }
            }
        );
    }

    #[tokio::test]
    async fn test_config_ordering() {
        let _h = testing::start();

        let context = TestContext::default();

        let system_config_path = context.user_config_path().expect("system_config_path");
        create_dir_all(&system_config_path)
            .await
            .expect("create_dir_all");

        let user_config_path = context.user_config_path().expect("user_config_path");
        create_dir_all(&user_config_path)
            .await
            .expect("create_dir_all");

        write_synced(
            system_config_path.join("config.toml"),
            "value = 1\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        write_synced(
            user_config_path.join("config.toml"),
            "value = 3\n\n[substate]\nsubvalue = 4\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 3,
                substate: TestSubstate { subvalue: 4 }
            }
        );
    }

    #[tokio::test]
    async fn test_config_partial_ordering() {
        let _h = testing::start();

        let context = TestContext::default();

        let system_config_path = context.system_config_path().expect("system_config_path");
        create_dir_all(&system_config_path)
            .await
            .expect("create_dir_all");

        let user_config_path = context.user_config_path().expect("user_config_path");
        create_dir_all(&user_config_path)
            .await
            .expect("create_dir_all");

        write_synced(
            system_config_path.join("config.toml"),
            "value = 1\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 2 }
            }
        );

        write_synced(
            user_config_path.join("config.toml"),
            "value = 3\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 3,
                substate: TestSubstate { subvalue: 2 }
            }
        );
    }

    #[tokio::test]
    async fn test_read_user_config_fragments() {
        let _h = testing::start();

        let context = TestContext::default();

        let user_config_path = context.user_config_path().expect("user_config_path");
        create_dir_all(user_config_path.join("config.toml.d"))
            .await
            .expect("create_dir_all");

        write_synced(
            user_config_path.join("config.toml"),
            "value = 1\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 2 }
            }
        );

        write_synced(
            user_config_path.join("config.toml.d/frag.toml"),
            "[substate]\nsubvalue = 3\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 3 }
            }
        );
    }

    #[tokio::test]
    async fn test_read_system_config_fragments() {
        let _h = testing::start();

        let context = TestContext::default();

        let system_config_path = context.system_config_path().expect("system_config_path");
        create_dir_all(system_config_path.join("config.toml.d"))
            .await
            .expect("create_dir_all");

        write_synced(
            system_config_path.join("config.toml"),
            "value = 1\n\n[substate]\nsubvalue = 2\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 2 }
            }
        );

        write_synced(
            system_config_path.join("config.toml.d/frag.toml"),
            "[substate]\nsubvalue = 3\n".as_bytes(),
        )
        .await
        .expect("write");

        let config = read_config(&context).await.expect("read_config");
        assert_eq!(
            config,
            TestState {
                value: 1,
                substate: TestSubstate { subvalue: 3 }
            }
        );
    }
}
