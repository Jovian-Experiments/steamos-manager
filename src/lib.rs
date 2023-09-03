use std::fs;
use std::{error::Error, future::pending};
use serde::{Serialize, Deserialize};
use zbus::{ConnectionBuilder, dbus_interface};

// We use s(teamos)m(anager) prefix on all types to help avoid conflicts

// Types of api we support, so far only dbus and script.
// For dbus type we call into other dbus apis specified.
// For script type we run a script and provide stdout, stderr, and exitcode back.
#[derive(Serialize, Deserialize, Debug)]
pub enum SmApiType {
    DBusType = 0,
    ScriptType = 1,
}

// SmDBusApi represents a dbus api to be called
// TODO: This may change to better match what zbus needs.
#[derive(Serialize, Deserialize, Debug)]
pub struct SmDbusApi {
    bus_name: String,
    object_path: String,
    method_name: String,
    // parameters: Vec
}

// SmScript represents a script to be executed
#[derive(Serialize, Deserialize, Debug)]
pub struct SmScript {
    path: String
}
// An SmOperation is what happens when an incoming dbus method is called.
// If the SmEntry type is DBusType this should be a DBusApi with the data neede.
// Otherwise it should be a script with the path to execute
#[derive(Serialize, Deserialize, Debug)]
pub enum SmOperation {
    SmScript(String),
    SmDbusApi(String, String, String)
}

// Each api config file contains one or more entries.
#[derive(Serialize, Deserialize, Debug)]
pub struct SmEntry {
    api_type: SmApiType,
    incoming: SmDbusApi, // TBD: The incoming zbus method for this entry
    outgoing: SmOperation, // TBD: Either the outgoing zbus method or a script to run
}

pub fn initialize_apis(path: String) -> Vec::<SmEntry>
{
    let mut res = Vec::<SmEntry>::new();
    for file in fs::read_dir(path).unwrap() {
        // Deserialize the file and add all entries to res
        
    }
    return res;
}