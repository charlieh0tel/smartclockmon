# CI runs these same targets; the workflow only calls "make ci".
#
# Every target says --workspace.  This is a virtual workspace, so that
# is already the default; saying it keeps the targets honest if a
# default-members is ever added.  One was, briefly, and it narrowed
# "cargo test" to a single crate: zero tests, exit 0, CI green while
# testing nothing.

CARGO ?= cargo

# The package version is asked of the binary being packaged rather than
# derived a second time here.  The binary is stamped at compile time by
# the library's build script, so the package, the --version output, the
# daemon's journal line, its info reply and the writer recorded in the
# log all carry one string.  Two builds of different code then cannot
# share a version, which matters because dpkg treats reinstalling an
# identical version as a no-op: the binaries change or they do not, and
# nothing from the outside says which.

.PHONY: all build ci fmt fmt-check clippy test test-hw doc docs clean deb install-service

all: build

build:
	$(CARGO) build --workspace --all-targets

# What CI runs.  Keep this the whole of it.
ci: fmt-check clippy test

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

# Everything that runs without a receiver.
test:
	$(CARGO) test --workspace --all-features

# Tests needing the attached receiver.  Stop smartclockd first: it holds
# the serial port open.  Never run in CI.
test-hw:
	$(CARGO) test --workspace --all-features -- --ignored

doc:
	$(CARGO) doc --workspace --no-deps --all-features

# Regenerate the documentation that is derived from the command table.
# A test fails if docs/commands.md and the table disagree.
docs:
	$(CARGO) run -q -p smartclock-cli -- commands > docs/commands.md

clean:
	$(CARGO) clean

# Requires cargo-deb: cargo install cargo-deb
# The deb carries all three binaries, so build the whole workspace first
# and package without rebuilding.  cargo-deb warns that the asset paths
# are not under target/release/ and so will not be built; that is the
# point, --no-build means they are already there.
deb:
	$(CARGO) build --release --workspace
# -q suppresses three warnings that cargo-deb emits every time and that
# nothing can act on: it only recognises asset paths beginning exactly
# "target/release/", and from crates/smartclockd the three binaries are
# at "../../target/release/".  It is telling us it will not build them,
# which is right -- the line above did, and --no-build says so.  The
# artifact path is still printed.
	$(CARGO) deb -p smartclockd --no-build -q \
	    --deb-version "$$(./target/release/smartclockd --version | awk '{print $$2}')"

install-service:
	install -m 0644 packaging/systemd/smartclockd.service /etc/systemd/system/
	install -m 0644 -b packaging/systemd/smartclockd.default /etc/default/smartclockd
	systemctl daemon-reload
