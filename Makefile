# CI runs these same targets; the workflow only calls "make ci".

CARGO ?= cargo

.PHONY: all build ci fmt fmt-check clippy test test-hw doc clean deb install-service

all: build

build:
	$(CARGO) build --all-targets

# What CI runs.  Keep this the whole of it.
ci: fmt-check clippy test

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --all-targets --all-features -- -D warnings

# Everything that runs without a receiver.
test:
	$(CARGO) test --all-features

# Tests needing the attached receiver.  Stop smartclockd first: it holds
# the serial port open.  Never run in CI.
test-hw:
	$(CARGO) test --all-features -- --ignored

doc:
	$(CARGO) doc --no-deps --all-features

clean:
	$(CARGO) clean

# Requires cargo-deb: cargo install cargo-deb
# The deb carries all three binaries, so build them before packaging and
# tell cargo-deb not to rebuild just the daemon.
deb: 
	$(CARGO) build --release
	$(CARGO) deb -p smartclockd --no-build

install-service:
	install -m 0644 packaging/systemd/smartclockd.service /etc/systemd/system/
	install -m 0644 -b packaging/systemd/smartclockd.default /etc/default/smartclockd
	systemctl daemon-reload
