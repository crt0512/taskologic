# taskologic-app
This is my "temporary" solution regarding the fact that I need to be able to have something like an "App" that I can open easy peasy on various machines to access my taskologic instance until I've implemented an actual web facing api (big task that I'd preffer to procastinate on for now)

## What is this?
This is the folder that holds the configurations to build a pseudo "App" that simply takes care of SSHing into the taskologic server and showing its TUI without me manually SSHing into it or using Terminal Emulators on my iPad or ancient Kindle Fire HD as an example.

## How to use this?
You will obviously need a set up build environment. Recommended is that you also have zig and cargo-zigbuild installed if you wanna build for more than just your exact os.

Needed no matter what you build:

- Rust via rustup (1.88 or newer), a C compiler and make 
  - C and make on Debian: `sudo apt install build-essential`
- Optional, for older systems and other architectures: 
  - `cargo install cargo-zigbuild --locked`
  - [zig](https://ziglang.org/download/) in your PATH. 
    - zig 0.16 complains about a "deprecated linker optimization setting" on every build, ignore it i dont care because I am a based sigma

What each platform wants on top of that is in its own section below.

---

### Getting Started

1. Clone my [Tacoshell Wrapper](https://github.com/crt0512/tacoshell) to wherever on your computer:
   1.  `git clone https://github.com/crt0512/tacoshell`
2. Copy the taskologic-app into the tacoshell folder: 
   1. `cp -r ./taskologic-app tacoshell/` 
3. Go into the tacoshell folder: `cd tacoshell`


Every command below takes `CONFIG=taskologic-app/taskologic.toml`, thats what makes it Taskologic instead of the demo shell mlol.
Whatever gets cooked up ends up in `target/`, make will print the file name of the taco that was just served at the end so you know what it made. 

Good to know:

- The build will warn about `credentials = "unsafe"` every time.
  -  Thats on purpose: Intention is that I will get annoyed by that enough to properly implement safe password storage for the various plattforms tacoshell targets.
- `taskologic-app --kiosk` starts it fullscreen with the on screen keyboard
- `taskologic-app --print-config` shows the baked in config with all defaults
- Want to try it before packaging? `make CONFIG=taskologic-app/taskologic.toml` then run `target/release/taskologic-app`

Now based on what you want to cook for / your os youll want to do either of these :

### Building for Debian / Ubuntu

- The .deb needs `dpkg-deb`
  -  comes with `dpkg-dev` on Debian (`sudo apt install dpkg-dev`).
- `make install` doesnt and just shoves the files rawdog style inplace where they should go.

- Build a .deb for your local system only :
  - `make deb CONFIG=taskologic-app/taskologic.toml`
  - Install it with `sudo apt install ./target/taskologic-app_1.0.2_amd64.deb` (exact name might differ)
    - That build will be bound to your glibc version, older distros wont like running this.
- Build for more people (needs zig and cargo-zigbuild, without falls back to your GLIBC version) :
  - Examples 
    - Debian 11 or newer : `make deb CONFIG=taskologic-app/taskologic.toml GLIBC=2.31`
    - arm64 (Raspberry Pie): `make deb CONFIG=taskologic-app/taskologic.toml TARGET=aarch64-unknown-linux-gnu GLIBC=2.31`

If you cant build for other plattforms make sure to add them as as a target. Raspberry Pie would be :
- `rustup target add aarch64-unknown-linux-gnu`

### Building for Android
Examples here assume you're on Debian or Gentoo like me.

Make sure you have these here:

- The Android SDK with build-tools 35 or newer, a platform and the NDK.
  - Android Studio's SDK manager can get you all of that. 
- `rustup target add aarch64-linux-android`
  - `rustup target add armv7-linux-androideabi`  if you plan on building for older devices too
- `zip` and Java's `keytool` are needed for the signing key
  - `sudo apt install zip default-jdk-headless`

Then:

- For somewhat recent Android devices
  - `make apk CONFIG=taskologic-app/taskologic.toml`
  - Runs on Android 7 and up, 64 bit arm devices only by default. To run on 32 bit devices too: 
  - Then add `ANDROID_TARGETS="aarch64-linux-android armv7-linux-androideabi"` to the make line, this way both binaries end up in the same apk <3
  - Its signed with Android Studio's debug key (will be made for you if there is none yet). 
  - For a key of your own: `ANDROID_KEYSTORE=path/to/key.jks ANDROID_KEY_ALIAS=name ANDROID_KEYSTORE_PASS=secret`
  - Android only installs an update if the version code went up. It comes from the version in taskologic.toml (1.0.2 becomes 1000002), so bump the version for every apk you hand out
  - `adb logcat -s tacoshell` shows what the app says if you want to debug
  - If the tablet is going to be hanged, Consider setting `enabled = true` in the `[kiosk]` section of taskologic.toml
    - adds the PIN and the locked login support.
- For old Android devices (like the ones I grew up with)
  - `make apk-legacy CONFIG=taskologic-app/taskologic.toml`
    - Should run on Android 4.0.3 and up (to a degree, at some point Android will complain about it being built for older versions of android)
  - Gives you `target/taskologic-app-1.0.2-legacy.apk`
    - Tested Working on a Kindle Fire HD 8.9"

### Building for MacOS
Not tested yet, cant decide which of my 7 Macs to try this on, (Pkg building cant be done on Debian), Needs Xcode's command line tools (`xcode-select --install`) and the rustup stuff from above:

- Build for the Mac you're on
  - `make pkg CONFIG=taskologic-app/taskologic.toml`

- For the other kind of recent Mac: 
  - Intel : `rustup target add x86_64-apple-darwin`
    - Then : `make pkg CONFIG=taskologic-app/taskologic.toml TARGET=x86_64-apple-darwin`
  - ARM : `rustup target add aarch64-apple-darwin`)
    - Then `make pkg CONFIG=taskologic-app/taskologic.toml TARGET=x86_64-apple-darwin`.

### Building for Windows
Not possible yet, tacoshell has no Windows build or installer so far because I am allergic to windows 

### Building for iOS / iPadOS
Not possible yet either coming one day
