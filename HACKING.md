This project is written in rust. For any unfamiliar with how that works the following should help:

To build you'll want a rust implementation for your laptop/device you are testing on.

https://wiki.archlinux.org/title/rust has some good guidelines for getting that sorted. but mostly boils down
to install rustup from the rustup package (if you're on arch) and use:

rustup default stable

to get an initial toolchain, etc.

Once you have that and cargo is in your path, to build and run this project use the following:

To build:

cargo build

To run:

cargo run

As far as IDEs go I find Visual Studio Code works pretty well for giving errors about things you
are changing and has plugins for vim mode, etc. if you are used to those keybindings. Most/all
IDEs that work with language servers should do that fine though.

For VS Code I use these extensions to help: rust and rust-analyzer
