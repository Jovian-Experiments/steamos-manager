/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;
use tokio::fs::read_to_string;
#[cfg(not(test))]
use tokio::sync::OnceCell;

#[cfg(not(test))]
use crate::hardware::is_deck;

#[cfg(not(test))]
static CONFIG: OnceCell<Option<PlatformConfig>> = OnceCell::const_new();

#[derive(Clone, Default, Deserialize, Debug)]
#[serde(default)]
pub(crate) struct PlatformConfig {
    pub factory_reset: Option<ResetConfig>,
    pub update_bios: Option<ScriptConfig>,
    pub update_dock: Option<ScriptConfig>,
    pub storage: Option<StorageConfig>,
    pub fan_control: Option<ServiceConfig>,
    pub tdp_limit: Option<RangeConfig<u32>>,
    pub gpu_clocks: Option<RangeConfig<u32>>,
    pub battery_charge_limit: Option<BatteryChargeLimitConfig>,
}

#[derive(Clone, Deserialize, Debug)]
pub(crate) struct RangeConfig<T: Clone> {
    pub min: T,
    pub max: T,
}

impl<T> Copy for RangeConfig<T> where T: Copy {}

#[derive(Clone, Default, Deserialize, Debug)]
pub(crate) struct ScriptConfig {
    pub script: PathBuf,
    #[serde(default)]
    pub script_args: Vec<String>,
}

#[derive(Clone, Default, Deserialize, Debug)]
pub(crate) struct ResetConfig {
    pub all: ScriptConfig,
    pub os: ScriptConfig,
    pub user: ScriptConfig,
}

#[derive(Clone, Deserialize, Debug)]
pub(crate) enum ServiceConfig {
    #[serde(rename = "systemd")]
    Systemd(String),
    #[serde(rename = "script")]
    Script {
        start: ScriptConfig,
        stop: ScriptConfig,
        status: ScriptConfig,
    },
}

#[derive(Clone, Default, Deserialize, Debug)]
pub(crate) struct StorageConfig {
    pub trim_devices: ScriptConfig,
    pub format_device: FormatDeviceConfig,
}

#[derive(Clone, Deserialize, Debug)]
pub(crate) struct BatteryChargeLimitConfig {
    pub suggested_minimum_limit: Option<i32>,
    pub hwmon_name: String,
    pub attribute: String,
}

#[derive(Clone, Default, Deserialize, Debug)]
pub(crate) struct FormatDeviceConfig {
    pub script: PathBuf,
    #[serde(default)]
    pub script_args: Vec<String>,
    pub label_flag: String,
    #[serde(default)]
    pub device_flag: Option<String>,
    #[serde(default)]
    pub validate_flag: Option<String>,
    #[serde(default)]
    pub no_validate_flag: Option<String>,
}

impl<T: Clone> RangeConfig<T> {
    #[allow(unused)]
    pub(crate) fn new(min: T, max: T) -> RangeConfig<T> {
        RangeConfig { min, max }
    }
}

impl PlatformConfig {
    #[cfg(not(test))]
    async fn load() -> Result<Option<PlatformConfig>> {
        if !is_deck().await? {
            // Non-Steam Deck platforms are not yet supported
            return Ok(None);
        }

        let config = read_to_string("/usr/share/steamos-manager/platforms/jupiter.toml").await?;
        Ok(Some(toml::from_str(config.as_ref())?))
    }
}

#[cfg(not(test))]
pub(crate) async fn platform_config() -> Result<&'static Option<PlatformConfig>> {
    CONFIG.get_or_try_init(PlatformConfig::load).await
}

#[cfg(test)]
pub(crate) async fn platform_config() -> Result<Option<PlatformConfig>> {
    let test = crate::testing::current();
    let config = test.platform_config.borrow().clone();
    Ok(config)
}

#[cfg(test)]
mod test {
    use super::*;

    #[tokio::test]
    async fn jupiter_valid() {
        let config = read_to_string("data/platforms/jupiter.toml")
            .await
            .expect("read_to_string");
        let res = toml::from_str::<PlatformConfig>(config.as_ref());
        assert!(res.is_ok(), "{res:?}");
    }
}
