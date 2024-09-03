# Overview

`scx_rust_scheduler` is a fully functional FIFO scheduler for the Linux kernel
that operates in user-space and it is 100% implemented in Rust.

It is based on `scx_rustland_core`, a framework that is specifically designed
to simplify the creation of user-space schedulers, leveraging the Linux
kernel's `sched_ext` feature and BPF.

The scheduler is intended to serve as a basic template for developers who want
to experiment with more advanced scheduling policies without having to directly
engage with low-level kernel internals.

# Requirements

In order to run this scheduler you need a kernel that supports `sched_ext`
(`CONFIG_SCHED_CLASS_EXT=y`).

You also need the following binaries/packages in order to build the scheduler:
 - cargo
 - rustc
 - bindgen
 - bpftool
 - libbpf

## CachyOS

CachyOS ships a `sched_ext` kernel by default, so you only need to install the
following user-space packages:

```
$ sudo pacman -S bpf libbpf rust clang pkgconf
```

## Ubuntu

If you are using Ubuntu, you can run the following commands to setup an
environment to build and test `scx_rust_scheduler`:

 - install a `sched_ext` Ubuntu kernel and all the required user-space
   dependencies:

```
$ sudo add-apt-repository -y --enable-source ppa:arighi/sched-ext
$ sudo apt update -y
$ sudo apt dist-upgrade -y
$ sudo apt install -y rustc cargo libbpf-dev pkg-config clang
```

 - reboot the system

# Getting Started

 - **Build the scheduler**:
```
$ cargo build
```

 - **Enable the scheduler**:
```
$ sudo ./target/debug/scx_rust_scheduler
Rust scheduler is enabled (CTRL+c to exit)
```

 - **Disable the scheduler**:
```
^C
Rust scheduler is disabled
EXIT: Scheduler unregistered from user space
```

# See also

 - [sched_ext schedulers and tools](https://github.com/sched-ext/scx)
 - [scx_rustland_core documentation](https://github.com/sched-ext/scx/blob/main/rust/scx_rustland_core/README.md)

# License

This software is licensed under the GNU General Public License version 2. See
the LICENSE file for details.
