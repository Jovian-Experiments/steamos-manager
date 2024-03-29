/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{anyhow, bail, ensure, Error, Result};
use std::str::FromStr;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tracing::error;

use crate::path;

const GPU_HWMON_PREFIX: &str = "/sys/class/drm/card0/device/hwmon";

const GPU_PERFORMANCE_LEVEL_PATH: &str =
    "/sys/class/drm/card0/device/power_dpm_force_performance_level";
const GPU_CLOCKS_PATH: &str = "/sys/class/drm/card0/device/pp_od_clk_voltage";

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
pub enum GPUPerformanceLevel {
    Auto = 1,
    Low = 2,
    High = 3,
    Manual = 4,
    ProfilePeak = 5,
}

impl TryFrom<u32> for GPUPerformanceLevel {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            x if x == GPUPerformanceLevel::Auto as u32 => Ok(GPUPerformanceLevel::Auto),
            x if x == GPUPerformanceLevel::Low as u32 => Ok(GPUPerformanceLevel::Low),
            x if x == GPUPerformanceLevel::High as u32 => Ok(GPUPerformanceLevel::High),
            x if x == GPUPerformanceLevel::Manual as u32 => Ok(GPUPerformanceLevel::Manual),
            x if x == GPUPerformanceLevel::ProfilePeak as u32 => {
                Ok(GPUPerformanceLevel::ProfilePeak)
            }
            _ => Err("No enum match for value {v}"),
        }
    }
}

impl FromStr for GPUPerformanceLevel {
    type Err = Error;
    fn from_str(input: &str) -> Result<GPUPerformanceLevel, Self::Err> {
        match input {
            "auto" => Ok(GPUPerformanceLevel::Auto),
            "low" => Ok(GPUPerformanceLevel::Low),
            "high" => Ok(GPUPerformanceLevel::High),
            "manual" => Ok(GPUPerformanceLevel::Manual),
            "peak_performance" => Ok(GPUPerformanceLevel::ProfilePeak),
            v => Err(anyhow!("No enum match for value {v}")),
        }
    }
}

impl ToString for GPUPerformanceLevel {
    fn to_string(&self) -> String {
        String::from(match self {
            GPUPerformanceLevel::Auto => "auto",
            GPUPerformanceLevel::Low => "low",
            GPUPerformanceLevel::High => "high",
            GPUPerformanceLevel::Manual => "manual",
            GPUPerformanceLevel::ProfilePeak => "peak_performance",
        })
    }
}

pub async fn get_gpu_performance_level() -> Result<GPUPerformanceLevel> {
    let level = fs::read_to_string(GPU_PERFORMANCE_LEVEL_PATH)
        .await
        .inspect_err(|message| error!("Error opening sysfs file for reading: {message}"))?;

    GPUPerformanceLevel::from_str(level.as_ref())
}

pub async fn set_gpu_performance_level(level: GPUPerformanceLevel) -> Result<()> {
    let mut myfile = File::create(GPU_PERFORMANCE_LEVEL_PATH)
        .await
        .inspect_err(|message| error!("Error opening sysfs file for writing: {message}"))?;

    let level: String = level.to_string();

    myfile
        .write_all(level.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;
    Ok(())
}

pub async fn set_gpu_clocks(clocks: i32) -> Result<()> {
    // Set GPU clocks to given value valid between 200 - 1600
    // Only used when GPU Performance Level is manual, but write whenever called.
    ensure!((200..=1600).contains(&clocks), "Invalid clocks");

    let mut myfile = File::create(GPU_CLOCKS_PATH)
        .await
        .inspect_err(|message| error!("Error opening sysfs file for writing: {message}"))?;

    let data = format!("s 0 {clocks}\n");
    myfile
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;

    let data = format!("s 1 {clocks}\n");
    myfile
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;

    myfile
        .write("c\n".as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to sysfs file: {message}"))?;
    Ok(())
}

pub async fn set_tdp_limit(limit: i32) -> Result<()> {
    // Set TDP limit given if within range (3-15)
    // Returns false on error or out of range
    ensure!((3..=15).contains(&limit), "Invalid limit");

    let mut dir = fs::read_dir(path(GPU_HWMON_PREFIX)).await?;
    let base = loop {
        let base = match dir.next_entry().await? {
            Some(entry) => entry.path(),
            None => bail!("hwmon not found"),
        };
        if fs::try_exists(base.join("power1_cap")).await? {
            break base;
        }
    };

    let data = format!("{limit}000000");

    let mut power1file = File::create(base.join("power1_cap"))
        .await
        .inspect_err(|message| {
            error!("Error opening sysfs power1_cap file for writing TDP limits {message}")
        })?;
    power1file
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to power1_cap file: {message}"))?;

    if let Ok(mut power2file) = File::create(base.join("power2_cap")).await {
        power2file
            .write(data.as_bytes())
            .await
            .inspect_err(|message| error!("Error writing to power2_cap file: {message}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing;
    use anyhow::anyhow;
    use tokio::fs::{create_dir_all, read_to_string, remove_dir, write};

    #[tokio::test]
    async fn test_set_tdp_limit() {
        let h = testing::start();

        assert_eq!(
            set_tdp_limit(2).await.unwrap_err().to_string(),
            anyhow!("Invalid limit").to_string()
        );
        assert_eq!(
            set_tdp_limit(20).await.unwrap_err().to_string(),
            anyhow!("Invalid limit").to_string()
        );
        assert!(set_tdp_limit(10).await.is_err());

        let hwmon = path(GPU_HWMON_PREFIX);
        create_dir_all(hwmon.as_path())
            .await
            .expect("create_dir_all");
        assert_eq!(
            set_tdp_limit(10).await.unwrap_err().to_string(),
            anyhow!("hwmon not found").to_string()
        );

        let hwmon = hwmon.join("hwmon5");
        create_dir_all(hwmon.join("power1_cap"))
            .await
            .expect("create_dir_all");
        create_dir_all(hwmon.join("power2_cap"))
            .await
            .expect("create_dir_all");
        assert_eq!(
            set_tdp_limit(10).await.unwrap_err().to_string(),
            anyhow!("Is a directory (os error 21)").to_string()
        );

        remove_dir(hwmon.join("power1_cap"))
            .await
            .expect("remove_dir");
        write(hwmon.join("power1_cap"), "0").await.expect("write");
        assert!(set_tdp_limit(10).await.is_ok());
        let power1_cap = read_to_string(hwmon.join("power1_cap"))
            .await
            .expect("power1_cap");
        assert_eq!(power1_cap, "10000000");

        remove_dir(hwmon.join("power2_cap"))
            .await
            .expect("remove_dir");
        write(hwmon.join("power2_cap"), "0").await.expect("write");
        assert!(set_tdp_limit(15).await.is_ok());
        let power1_cap = read_to_string(hwmon.join("power1_cap"))
            .await
            .expect("power1_cap");
        assert_eq!(power1_cap, "15000000");
        let power2_cap = read_to_string(hwmon.join("power2_cap"))
            .await
            .expect("power2_cap");
        assert_eq!(power2_cap, "15000000");
    }
}
