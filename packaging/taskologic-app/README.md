# taskologic-app
This is my temporary solution regarding the fact that I need to be able to have something like an "App" that I can open easy peasy on various machines to access my taskologic instance until I've implemented an actual web facing api (big task that I'd preffer to procastinate on for now)

## What is this?
This is the folder that holds the configurations to build a pseudo "App" that simply takes care of SSHing into the taskologic server and showing its TUI without me manually SSHing into it or using Terminal Emulators on my iPad as an example.

## How to use this?
You will obviously need a set up build environement, recommmended si that you also have zig and cargo-zigbuild installed if you wanna build for more than just your exact os.

- Rust via rustup, plus a C compiler and make (on Debian: `sudo apt install build-essential`)
- For .deb files: `dpkg-deb`, which comes with `dpkg-dev` on Debian
- Optional, for older systems and other architectures: `cargo install cargo-zigbuild --locked` and [zig](https://ziglang.org/download/) somewhere in your PATH

1. Clone my [Tacoshell Wrapper](https://github.com/crt0512/tacoshell) to wherever on your computer: `git clone https://github.com/crt0512/tacoshell`
2. Copy the taskologic-app into the tacoshell folder: `cp -r path/to/taskologic/packaging/taskologic-app tacoshell/`
3. Go into the tacoshell folder: `cd tacoshell`

Now based on what you want to build for / your os youll want to do either of these :

### Building for Debian
- Build deb file for your local system only :
  - `make deb CONFIG=taskologic-app/taskologic.toml`
  - The .deb ends up in `target/` (make prints its name at the end), install it with `sudo apt install ./target/taskologic-app_<version>_<arch>.deb`
  - It needs at least the glibc of the machine you built it on, so it won't run on older Debian/Ubuntu versions
  - Don't want a .deb? `make install CONFIG=taskologic-app/taskologic.toml` puts it straight into /usr/local (and `make uninstall CONFIG=taskologic-app/taskologic.toml` takes it out again)
- Build for something else (needs zig and cargo-zigbuild, without them it just builds for your system like above) :
  - Older systems: `make deb CONFIG=taskologic-app/taskologic.toml GLIBC=2.31` runs on Debian 11 / Ubuntu 20.04 and newer
  - arm64: `rustup target add aarch64-unknown-linux-gnu` once, then `make deb CONFIG=taskologic-app/taskologic.toml TARGET=aarch64-unknown-linux-gnu GLIBC=2.31`


### Building for MacOS
Not tested yet. On a Mac, with Xcode's command line tools (`xcode-select --install`):
- `make pkg CONFIG=taskologic-app/taskologic.toml`
- Builds for the Mac you're on (Apple Silicon or Intel), the .pkg ends up in `target/`


### Building for Windows
Not possible yet, tacoshell has no Windows build or installer so far.
