use steamos_manager::{self, SmEntry, initialize_apis};

fn main() {
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things (change brightness, etc.) In order to do that it reads a folder of dbus api
    // configuration files and exposes each configuration with the api in the config file. In order
    // to know what to do with each it gets the information from the same config file about whether
    // to run a script or call some other dbus api.

    let mut manager_apis: Vec<SmEntry> = initialize_apis("/usr/share/steamos-manager".to_string());
}
