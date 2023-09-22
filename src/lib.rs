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

use std::fs;
use std::{error::Error, future::pending};
use std::collections::HashMap;
use std::convert::TryFrom;

use serde::{Serialize, Deserialize};
use zbus::{Connection, ConnectionBuilder, Result, dbus_interface, zvariant::{from_slice, to_bytes, EncodingContext, Value}};

pub mod manager;

// We use s(teamos)m(anager) prefix on all types to help avoid conflicts

// Types of api we support, so far only dbus and script.
// For script type we run a script and provide stdout, stderr, and exitcode back.
// For SysFS type we manipulate sys fs values, on/off or setting a specific value
#[derive(Serialize, Deserialize, Debug)]
pub enum SmApiType {
    ScriptType = 1,
    SysFSType = 2, 
}

// SmDBusApi represents a dbus api to be called
// TODO: This may change to better match what zbus needs.
#[derive(Serialize, Deserialize, Debug)]
pub struct SmDbusApi {
    bus_name: String, // The servcive name, i.e. org.freedesktop.Notifications
    object_path: String, // The object path, i.e. /org/freedesktop/Notifications
    interface_name: String, // The interface used i.e. org.freedesktop.Notifications
    method_name: String // The method name, i.e. Notify
}

// SmScript represents a script to be executed
#[derive(Serialize, Deserialize, Debug)]
pub struct SmScript {
    path: String
}

// SmSysfs represents a read/write to a sysfs path or paths
#[derive(Serialize, Deserialize, Debug)]
pub struct SmSysfs {
    path: String,
    // value: zbus::zvariant::Value<'a>
}
// An SmOperation is what happens when an incoming dbus method is called.
// If the SmEntry type is DBusType this should be a DBusApi with the data neede.
// Otherwise it should be a script with the path to execute
#[derive(Serialize, Deserialize, Debug)]
pub enum SmOperation {
    SmScript(String),
    SmDbusApi(String, String, String),
    // SmSysfs(String, zbus::zvariant::Value)
}

// Each api config file contains one or more entries.
#[derive(Serialize, Deserialize, Debug)]
pub struct SmEntry {
    api_type: SmApiType,
    incoming: SmDbusApi, // TBD: The incoming zbus method for this entry
    outgoing: SmOperation, // TBD: Either the outgoing zbus method or a script to run
}

pub fn initialize_apis(path: String) -> Result<Vec::<SmEntry>>
{
    let res = Vec::<SmEntry>::new();
    // for file in fs::read_dir(path)? {
        // Deserialize the file and add SmEntry to res
    // }
    return Ok(res);
}

pub fn create_dbus_apis(connection: zbus::Connection, entries: Vec::<SmEntry>) -> bool
{
    // Create each of the given apis as dbus methods that users, etc.
    // can use to call into us.
    // for api in entries
    // {
        // 

    // }
    return true;
}
