This project is written in [Rust](https://www.rust-lang.org/). For any
unfamiliar with how that works the following should help:

# Building

To build you'll want a Rust implementation for your laptop/device you are testing on.

The [Arch wiki article](https://wiki.archlinux.org/title/rust) has some good
guidelines for getting that sorted, but mostly boils down to installing
`rustup` from the `rustup` package (if you're on Arch) and running `rustup
default stable` to get an initial toolchain.

Once you have that and `cargo` is in your path, to build the project you can
use `cargo build`. To run tests you can use `cargo test`.

Note that Arch also provides the `rust` package for a system-managed Rust
installation, which is also sufficient for development on this project.

# Developing

As far as IDEs go, Visual Studio Code works pretty well for giving errors about
things you are changing and has plugins for vim mode, etc. if you are used to
those keybindings. Most/all IDEs that work with language servers should do that
fine though.

For VS Code, these extensions help: `rust` and `rust-analyzer`.

Before committing code, please run `cargo fmt` to make sure that your code
matches the preferred code style, and `cargo clippy` can help with common
mistakes and idiomatic code.
