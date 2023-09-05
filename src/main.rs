use std::io::ErrorKind;

use steamos_manager::*;

use zbus::{Connection, Result};

#[async_std::main]
async fn main() -> Result<()>
{
    // This daemon is responsible for creating a dbus api that steam client can use to do various OS
    // level things (change brightness, etc.) In order to do that it reads a folder of dbus api
    // configuration files and exposes each configuration with the api in the config file. In order
    // to know what to do with each it gets the information from the same config file about whether
    // to run a script or call some other dbus api.

    let session_connection = Connection::session().await?;
    session_connection.request_name("com.steampowered.SteamOSManager").await?;

    let result = initialize_apis("/usr/share/steamos-manager".to_string());
    match result {
        Ok(manager_apis) => {
            let worked: bool = create_dbus_apis(session_connection, manager_apis);
        }
        Err(error) => {
            println!("There was an error reading configuration files, doing nothing. {:?}", error);
        }
    }
    
    loop
    {
        
    }
}
