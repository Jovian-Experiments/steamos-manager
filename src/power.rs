/*
 * Copyright © 2023 Collabora Ltd.
 * Copyright © 2024 Valve Software
 *
 * SPDX-License-Identifier: MIT
 */

use anyhow::{ensure, Result};
use tokio::{fs::File, io::AsyncWriteExt};
use tracing::error;

const POWER1_CAP_PATH: &str = "/sys/class/hwmon/hwmon5/power1_cap";
const POWER2_CAP_PATH: &str = "/sys/class/hwmon/hwmon5/power2_cap";

const GPU_PERFORMANCE_LEVEL_PATH: &str =
    "/sys/class/drm/card0/device/power_dpm_force_performance_level";
const GPU_CLOCKS_PATH: &str = "/sys/class/drm/card0/device/pp_od_clk_voltage";

pub async fn set_gpu_performance_level(level: i32) -> Result<()> {
    // Set given GPU performance level
    // Levels are defined below
    // return true if able to write, false otherwise or if level is out of range, etc.
    let levels = ["auto", "low", "high", "manual", "peak_performance"];
    ensure!(
        level >= 0 && level < levels.len() as i32,
        "Invalid performance level"
    );

    let mut myfile = File::create(GPU_PERFORMANCE_LEVEL_PATH)
        .await
        .inspect_err(|message| error!("Error opening sysfs file for writing: {message}"))?;

    myfile
        .write_all(levels[level as usize].as_bytes())
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

    let mut power1file = File::create(POWER1_CAP_PATH).await.inspect_err(|message| {
        error!("Error opening sysfs power1_cap file for writing TDP limits {message}")
    })?;

    let mut power2file = File::create(POWER2_CAP_PATH).await.inspect_err(|message| {
        error!("Error opening sysfs power2_cap file for wtriting TDP limits {message}")
    })?;

    // Now write the value * 1,000,000
    let data = format!("{limit}000000");
    power1file
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to power1_cap file: {message}"))?;
    power2file
        .write(data.as_bytes())
        .await
        .inspect_err(|message| error!("Error writing to power2_cap file: {message}"))?;
    Ok(())
}
