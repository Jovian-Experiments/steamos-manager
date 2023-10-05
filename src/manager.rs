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

use std::ffi::OsStr;
use subprocess::{ExitStatus::Exited, Popen, PopenConfig, PopenError};
use zbus_macros::dbus_interface;
pub struct SMManager {
}

fn script_exit_code(argv: &[impl AsRef<OsStr>]) -> Result<bool, PopenError> {
    // Run given script and return true on success
    let mut process = Popen::create(argv, PopenConfig::default())?;
    let exit_status = process.wait()?;
    Ok(exit_status == Exited(0))
}

fn run_script(name: &str, argv: &[impl AsRef<OsStr>]) -> bool {
    // Run given script to get exit code and return true on success.
    // Return false on failure, but also print an error if needed
    match script_exit_code(argv) {
        Ok(value) => value,
        Err(err) => { println!("Error running {} {}", name, err); false }
    }
}

#[dbus_interface(name = "com.steampowered.SteamOSManager1")]
impl SMManager {
    const API_VERSION: u32 = 1;


    async fn say_hello(&self, name: &str) -> String {
        format!("Hello {}!", name)
    }
    
    async fn factory_reset(&self) -> bool {
        // Run steamos factory reset script and return true on success
        run_script("factory reset", &["steamos-factory-reset-config"])
    }

    async fn disable_wifi_power_management(&self) -> bool {
        // Run polkit helper script and return true on success
        run_script("disable wifi power management", &["/usr/bin/steamos-polkit-helpers/steamos-disable-wireless-power-management"])
    }
    
    async fn enable_fan_control(&self, enable: bool) -> bool {
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        if enable {
            run_script("enable fan control", &["systemctl", "start", "jupiter-fan-control.service"])
        } else {
            run_script("disable fan control", &["systemctl", "stop", "jupiter-fan-control.service"])
        }
    }

    async fn hardware_check_support(&self) -> bool {
        // Run jupiter-check-support note this script does exit 1 for "Support: No" case
        // so no need to parse output, etc.
        run_script("check hardware support", &["jupiter-check-support"])
    }

    /// A version property.
    #[dbus_interface(property)]
    async fn version(&self) -> u32 {
        SMManager::API_VERSION
    }
}

