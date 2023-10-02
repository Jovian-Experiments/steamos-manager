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
use subprocess::{ExitStatus::Exited, Popen, PopenConfig, Redirection};
use zbus_macros::dbus_interface;
use zbus::{ObjectServer, SignalContext, MessageHeader};
pub struct SMManager {
}

fn run_script(argv: &[impl AsRef<OsStr>]) -> bool {
    // Run given script and return true on success
    let mut process = Popen::create(argv, PopenConfig {
        stdout: Redirection::Pipe, ..Default::default()
    }).unwrap();
    let (_out, _err) = process.communicate(None).unwrap();
    if let Some(exit_status) = process.poll() {
        return exit_status == Exited(0);
    } else {
        return false;
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
        return run_script(&["steamos-factory-reset-config"]);
    }

    async fn disable_wifi_power_management(&self) -> bool {
        // Run  what steamos-polkit-helpers/steamos-disable-wifi-power-management does
        return run_script(&["iwconfig", "wlan0", "power", "off"]);
    }
    
    async fn enable_fan_control(&self, enable: bool) -> bool {
        // Run what steamos-polkit-helpers/jupiter-fan-control does
        if enable {
            return run_script(&["systemctl", "start", "jupiter-fan-control.service"]);
        } else {
            return run_script(&["systemctl", "stop", "jupiter-fan-control.service"]);
        }
    }

    async fn hardware_check_support(&self) -> bool {
        // Run jupiter-check-support note this script does exit 1 for "Support: No" case
        // so no need to parse output, etc.
        return run_script(&["jupiter-check-support"])
    }

    /// A version property.
    #[dbus_interface(property)]
    async fn version(&self) -> u32 {
        SMManager::API_VERSION
    }
}

