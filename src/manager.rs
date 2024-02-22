/*
 * Copyright © 2023 Collabora Ltd.
 *
 * SPDX-License-Identifier: MIT
 *
 * Permission is hereby granted, free of charge, to any person obtaining
 * a copy of this software and associated documentation files (the
 * "Software"), to deal in the Software without restriction, including
 * without limitation the rights to use, copy, modify, merge, publish,
 * distribute, sublicense, and/or sell copies of the Software, and to
 * permit persons to whom the Software is furnished to do so, subject to
 * the following conditions:
 *
 * The above copyright notice and this permission notice shall be included
 * in all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
 * EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
 * MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
 * IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
 * CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT,
 * TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE
 * SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

use std::{ fs, ffi::OsStr, fmt, os::fd::{FromRawFd, IntoRawFd} };
use tokio::{fs::File, io::AsyncWriteExt, process::Command};
use zbus::zvariant::OwnedFd;
use zbus_macros::dbus_interface;

#[derive(PartialEq, Debug, Copy, Clone)]
#[repr(u32)]
enum WifiDebugMode {
    Off,
    On,
}

impl TryFrom<u32> for WifiDebugMode {
    type Error = &'static str;
    fn try_from(v: u32) -> Result<Self, Self::Error>
    {
        match v {
            x if x == WifiDebugMode::Off as u32 => Ok(WifiDebugMode::Off),
            x if x == WifiDebugMode::On as u32 => Ok(WifiDebugMode::On),
            _ => { Err("No enum match for value {v}") },
        }

    }

}

impl fmt::Display for WifiDebugMode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WifiDebugMode::Off => write!(f, "Off"),
            WifiDebugMode::On => write!(f, "On"),
        }
    }
}

pub struct SMManager {
    wifi_debug_mode: WifiDebugMode,
    // Whether we should use trace-cmd or not.
    // True on galileo devices, false otherwise
    should_trace: bool,
}

impl SMManager
{
    pub fn new() -> Self
    {
        SMManager {
            wifi_debug_mode: WifiDebugMode::Off,
            should_trace: is_galileo().unwrap(),
        }
    }
}

impl Default for SMManager
{
    fn default() -> Self
    {
        SMManager::new()
    }

}

const OVERRIDE_CONTENTS: &str =
"[Service]
ExecStart=
ExecStart=/usr/lib/iwd/iwd -d
";
const OVERRIDE_FOLDER: &str = "/etc/systemd/system/iwd.service.d";
const OVERRIDE_PATH: &str = "/etc/systemd/system/iwd.service.d/override.conf";
// Only use one path for output for now. If needed we can add a timestamp later
// to have multiple files, etc.
const OUTPUT_FILE: &str = "/var/log/wifitrace.dat";
const MIN_BUFFER_SIZE: u32 = 100;

const BOARD_NAME_PATH: &str = "/sys/class/dmi/id/board_name";
const GALILEO_NAME: &str = "Galileo";

fn is_galileo() -> std::io::Result<bool>
{
    let mut board_name = fs::read_to_string(BOARD_NAME_PATH)?;
    board_name = board_name.trim().to_string();

    let matches = board_name == GALILEO_NAME;
    Ok(matches)
}

async fn script_exit_code(
    executable: &str,
    args: &[impl AsRef<OsStr>],
) -> std::io::Result<bool> {
    // Run given script and return true on success
    let mut child = Command::new(executable)
        .args(args)
        .spawn()?;
    let status = child.wait().await?;
    Ok(status.success())
}

async fn run_script(name: &str, executable: &str, args: &[impl AsRef<OsStr>]) -> std::io::Result<bool> {
    // Run given script to get exit code and return true on success.
    // Return false on failure, but also print an error if needed
    match script_exit_code(executable, args).await {
        Ok(value) => Ok(value),
        Err(err) => {
            println!("Error running {} {}", name, err);
            Err(err)
        }
    }
}

async fn script_output(
    executable: &str,
    args: &[impl AsRef<OsStr>],
) -> Result<String, Box<dyn std::error::Error>> {
    // Run given command and return the output given
    let output = Command::new(executable).args(args).output();

    let output = output.await?;

    let s = match std::str::from_utf8(&output.stdout) {
        Ok(v) => v,
        Err(e) => panic!("Invalid UTF-8 sequence: {}", e),
    };
    Ok(s.to_string())
}

async fn setup_iwd_config(want_override: bool) -> Result<(), std::io::Error>
{
    // Copy override.conf file into place or out of place depending 
    // on install value

    if want_override {
        // Copy it in
        // Make sure the folder exists
        tokio::fs::create_dir_all(OVERRIDE_FOLDER).await?;
        // Then write the contents into the file
        tokio::fs::write(OVERRIDE_PATH, OVERRIDE_CONTENTS).await
    } else {
        // Delete it
        tokio::fs::remove_file(OVERRIDE_PATH).await
    }
}

async fn restart_iwd() -> std::io::Result<bool>
{
    // First reload systemd since we modified the config most likely
    // othorwise we wouldn't be restarting iwd.
    match run_script("reload systemd", "systemctl", &["daemon-reload"]).await {
        Ok(value) => {
            if value {
                // worked, now restart iwd
                run_script("restart iwd", "systemctl", &["restart", "iwd"]).await
            } else {
                // reload failed
                println!("restart_iwd: reload systemd failed somehow");
                Ok(false)
            }
        },
        Err(message) => {
            println!("restart_iwd: reload systemd got an error {message}");
            Err(message)
        }
    }
}

async fn stop_tracing(should_trace: bool) -> std::io::Result<bool>
{
    if !should_trace {
        return Ok(true);
    }

    // Stop tracing and extract ring buffer to disk for capture
    run_script("stop tracing", "trace-cmd", &["stop"]).await?;
    // stop tracing worked
    run_script("extract traces", "trace-cmd", &["extract", "-o", OUTPUT_FILE]).await
}

async fn start_tracing(buffer_size:u32, should_trace: bool) -> std::io::Result<bool>
{
    if !should_trace {
        return Ok(true);
    }

    // Start tracing
    let size_str = format!("{}", buffer_size);
    run_script("start tracing", "trace-cmd", &["start", "-e", "ath11k_wmi_diag", "-b", &size_str]).await
}

#[dbus_interface(name = "com.steampowered.SteamOSManager1")]
impl SMManager {
    const API_VERSION: u32 = 1;

    async fn say_hello(&self, name: &str) -> String {
        format!("Hello {}!", name)
    }

    async fn factory_reset(&self) -> bool {
        // Run steamos factory reset script and return true on success
        match run_script("factory reset", "steamos-factory-reset-config", &[""]).await {
            Ok(value) => { value },
            Err(_) => { false }
        }
    }

    async fn disable_wifi_power_management(&self) -> bool {
        // Run polkit helper script and return true on success
        match run_script(
            "disable wifi power management",
            "/usr/bin/steamos-polkit-helpers/steamos-disable-wireless-power-management",
            &[""],
        )
        .await {
            Ok(value) => { value },
            Err(_) => { false }
        }
    }

    async fn enable_fan_control(&self, enable: bool) -> bool {
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        if enable {
            match run_script(
                "enable fan control",
                "systemctl",
                &["start", "jupiter-fan-control.service"],
            )
            .await {
                Ok(value) => { value },
                Err(_) => { false }
            }
        } else {
            match run_script(
                "disable fan control",
                "systemctl",
                &["stop", "jupiter-fan-control.service"],
            )
            .await {
                Ok(value) => { value },
                Err(_) => { false }
            }
        }
    }

    async fn hardware_check_support(&self) -> bool {
        // Run jupiter-check-support note this script does exit 1 for "Support: No" case
        // so no need to parse output, etc.
        match run_script("check hardware support", "jupiter-check-support", &[""]).await {
            Ok(value) => { value },
            Err(_) => { false }
        }
    }

    async fn read_als_calibration(&self) -> f32 {
        // Run script to get calibration value
        let result = script_output(
            "/usr/bin/steamos-polkit-helpers/jupiter-get-als-gain",
            &[""],
        )
        .await;
        let mut value: f32 = -1.0;
        match result {
            Ok(as_string) => value = as_string.trim().parse().unwrap(),
            Err(message) => println!("Unable to run als calibration script : {}", message),
        }

        value
    }

    async fn update_bios(&self) -> bool {
        // Update the bios as needed
        // Return true if the script was successful (though that might mean no update was needed), false otherwise
        match run_script(
            "update bios",
            "/usr/bin/steamos-polkit-helpers/jupiter-biosupdate",
            &["--auto"],
        )
        .await {
            Ok(value) => { value },
            Err(_) => { false }
        }
    }

    async fn update_dock(&self) -> bool {
        // Update the dock firmware as needed
        // Retur true if successful, false otherwise
        match run_script(
            "update dock firmware",
            "/usr/bin/steamos-polkit-helpers/jupiter-dock-updater",
            &[""],
        )
        .await {
            Ok(value) => { value },
            Err(_) => { false }
        }
    }

    async fn trim_devices(&self) -> bool {
        // Run steamos-trim-devices script
        // return true on success, false otherwise
        match run_script(
            "trim devices",
            "/usr/bin/steamos-polkit-helpers/steamos-trim-devices",
            &[""],
        )
        .await {
            Ok(value) => { value },
            Err(_) => { false }
        }
    }

    async fn format_sdcard(&self) -> bool {
        // Run steamos-format-sdcard script
        // return true on success, false otherwise
        match run_script(
            "format sdcard",
            "/usr/bin/steamos-polkit-helpers/steamos-format-sdcard",
            &[""],
        )
        .await {
            Ok(value) => { value },
            Err(_) => { false }
        }
    }

    async fn set_gpu_performance_level(&self, level: i32) -> bool {
        // Set given level to sysfs path /sys/class/drm/card0/device/power_dpm_force_performance_level
        // Levels are defined below
        // return true if able to write, false otherwise or if level is out of range, etc.
        let levels = ["auto", "low", "high", "manual", "peak_performance"];
        if level < 0 || level >= levels.len() as i32 {
            return false;
        }

        // Open sysfs file
        let result =
            File::create("/sys/class/drm/card0/device/power_dpm_force_performance_level").await;
        let mut myfile;
        match result {
            Ok(f) => myfile = f,
            Err(message) => {
                println!("Error opening sysfs file for writing {message}");
                return false;
            }
        };

        // write value
        let result = myfile.write_all(levels[level as usize].as_bytes()).await;
        match result {
            Ok(_worked) => true,
            Err(message) => {
                println!("Error writing to sysfs file {message}");
                false
            }
        }
    }

    async fn set_gpu_clocks(&self, clocks: i32) -> bool {
        // Set gpu clocks to given value valid between 200 - 1600
        // Only used when Gpu Performance Level is manual, but write whenever called.
        // Writes value to /sys/class/drm/card0/device/pp_od_clk_voltage
        if !(200..=1600).contains(&clocks) {
            return false;
        }

        let result = File::create("/sys/class/drm/card0/device/pp_od_clk_voltage").await;
        let mut myfile;
        match result {
            Ok(f) => myfile = f,
            Err(message) => {
                println!("Error opening sysfs file for writing {message}");
                return false;
            }
        };

        // write value
        let data = format!("s 0 {clocks}\n");
        let result = myfile.write(data.as_bytes()).await;
        match result {
            Ok(_worked) => {
                let data = format!("s 1 {clocks}\n");
                let result = myfile.write(data.as_bytes()).await;
                match result {
                    Ok(_worked) => {
                        let result = myfile.write("c\n".as_bytes()).await;
                        match result {
                            Ok(_worked) => true,
                            Err(message) => {
                                println!("Error writing to sysfs file {message}");
                                false
                            }
                        }
                    }
                    Err(message) => {
                        println!("Error writing to sysfs file {message}");
                        false
                    }
                }
            }
            Err(message) => {
                println!("Error writing to sysfs file {message}");
                false
            }
        }
    }

    async fn set_tdp_limit(&self, limit: i32) -> bool {
        // Set TDP limit given if within range (3-15)
        // Returns false on error or out of range
        // Writes value to /sys/class/hwmon/hwmon5/power[12]_cap
        if !(3..=15).contains(&limit) {
            return false;
        }

        let result = File::create("/sys/class/hwmon/hwmon5/power1_cap").await;
        let mut power1file;
        match result {
            Ok(f) => power1file = f,
            Err(message) => {
                println!("Error opening sysfs power1_cap file for writing TDP limits {message}");
                return false;
            }
        };

        let result = File::create("/sys/class/hwmon/hwmon5/power2_cap").await;
        let mut power2file;
        match result {
            Ok(f) => power2file = f,
            Err(message) => {
                println!("Error opening sysfs power2_cap file for wtriting TDP limits {message}");
                return false;
            }
        };

        // Now write the value * 1,000,000
        let data = format!("{limit}000000");
        let result = power1file.write(data.as_bytes()).await;
        match result {
            Ok(_worked) => {
                let result = power2file.write(data.as_bytes()).await;
                match result {
                    Ok(_worked) => true,
                    Err(message) => {
                        println!("Error writing to power2_cap file: {message}");
                        false
                    }
                }
            }
            Err(message) => {
                println!("Error writing to power1_cap file: {message}");
                false
            }
        }
    }

    async fn get_als_integration_time_file_descriptor(
        &self,
    ) -> Result<zbus::zvariant::OwnedFd, zbus::fdo::Error> {
        // Get the file descriptor for the als integration time sysfs path
        // /sys/devices/platform/AMDI0010:00/i2c-0/i2c-PRP0001:01/iio:device0/in_illuminance_integration_time
        // Return -1 on error
        let result = File::create("/sys/devices/platform/AMDI0010:00/i2c-0/i2c-PRP0001:01/iio:device0/in_illuminance_integration_time").await;
        match result {
            Ok(f) => {
                let raw = f.into_std().await.into_raw_fd();
                unsafe {
                    let fd: OwnedFd = OwnedFd::from_raw_fd(raw);
                    Ok(fd)
                }
            }
            Err(message) => {
                println!("Error opening sysfs file for giving file descriptor {message}");
                Err(zbus::fdo::Error::IOError(message.to_string()))
            }
        }
    }

    async fn get_wifi_debug_mode(&mut self) -> u32 {
        // Get the wifi debug mode
        self.wifi_debug_mode as u32
    }

    async fn set_wifi_debug_mode(&mut self, mode: u32, buffer_size: u32) -> bool {
        // Set the wifi debug mode to mode, using an int for flexibility going forward but only
        // doing things on 0 or 1 for now
        // Return false on error

        let wanted_mode = WifiDebugMode::try_from(mode);
        match wanted_mode {
            Ok(WifiDebugMode::Off) => {
                // If mode is 0 disable wifi debug mode
                // Stop any existing trace and flush to disk.
                match stop_tracing(self.should_trace).await {
                    Ok(result) => {
                        if result {
                            // Stop_tracing was successful
                            match setup_iwd_config(false).await {
                                Ok(_) => {
                                    // setup_iwd_config false worked
                                    match restart_iwd().await {
                                        Ok(value) => {
                                            if value {
                                                // restart iwd worked
                                                self.wifi_debug_mode = WifiDebugMode::Off;
                                            } else {
                                                // restart_iwd failed
                                                println!("restart_iwd failed somehow, check log above");
                                                return false;
                                            }
                                        },
                                        Err(message) => {
                                            println!("restart_iwd got an error {message}");
                                            return false;
                                        }
                                    }
                                },
                                Err(message) => {
                                    println!("setup_iwd_config false got an error somehow {message}");
                                    return false;
                                }
                            }
                        } else {
                            println!("stop_tracing command failed somehow, bailing");
                            return false;

                        }
                    },
                    Err(message) => {
                        println!("stop_tracing command had an error {message}");
                        return false;
                    }
                }
            },
            Ok(WifiDebugMode::On) => {
                // If mode is 1 enable wifi debug mode
                if buffer_size < MIN_BUFFER_SIZE {
                    return false;
                }

                match setup_iwd_config(true).await {
                    Ok(_) => {
                        // setup_iwd_config worked
                        match restart_iwd().await {
                            Ok(value) => {
                                if value {
                                    // restart_iwd worked
                                    match start_tracing(buffer_size, self.should_trace).await {
                                        Ok(value) => {
                                            if value {
                                                // start_tracing worked
                                                self.wifi_debug_mode = WifiDebugMode::On;
                                            } else {
                                                // start_tracing failed
                                                println!("start_tracing failed somehow");
                                                return false;
                                            }
                                        },
                                        Err(message) => {
                                            println!("start_tracing got an error {message}");
                                            return false;
                                        }
                                    }
                                } else {
                                    println!("restart_iwd failed somehow");
                                    return false;
                                }
                            },
                            Err(message) => {
                                println!("restart_iwd got an error {message}");
                                return false;
                            }
                        }
                    },
                    Err(message) => {
                        println!("setup_iwd_config true got an error somehow {message}");
                        return false;
                    }
                }
            },
            Err(_) => {
                // Invalid mode requested, more coming later, but add this catch-all for now
                println!("Invalid wifi debug mode {mode} requested"); 
                return false;
            },
        }

        true
    }

    /// A version property.
    #[dbus_interface(property)]
    async fn version(&self) -> u32 {
        SMManager::API_VERSION
    }
}
