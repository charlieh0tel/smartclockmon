# CI runs these same targets; the workflow only calls "make ci".
#
# Every target says --workspace.  default-members points at smartclockd
# so cargo deb and cargo run resolve without -p, but that also narrows
# every other cargo command to that one crate: a bare "cargo test" runs
# zero tests and still exits 0, which once made CI pass while testing
# nothing.

CARGO ?= cargo

.PHONY: all build ci fmt fmt-check clippy test test-hw doc clean deb install-service

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

clean:
	$(CARGO) clean

# Requires cargo-deb: cargo install cargo-deb
# The deb carries all three binaries, so build the whole workspace first
# and package without rebuilding.  cargo-deb warns that the asset paths
# are not under target/release/ and so will not be built; that is the
# point, --no-build means they are already there.
deb:
	$(CARGO) build --release --workspace
	$(CARGO) deb --no-build

install-service:
	install -m 0644 packaging/systemd/smartclockd.service /etc/systemd/system/
	install -m 0644 -b packaging/systemd/smartclockd.default /etc/default/smartclockd
	systemctl daemon-reload
