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

.PHONY: all build ci fmt fmt-check clippy test test-hw doc docs clean deb release install-service

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
# -q keeps the output to the artifact path.
	$(CARGO) deb -p smartclockd --no-build -q \
	    --deb-version "$$(./target/release/smartclockd --version | awk '{print $$2}')"

# Cut a release: one version, in both places that must agree, tagged.
#
# The package version and the changelog are what apt compares, and the
# build stamp reads the tag, so all three have to move together.  Doing
# it by hand means one of them is eventually forgotten and an upgrade
# silently is not one.  Refuses a dirty tree, since the tag would name a
# commit that does not contain what was built.
#
#     make release VERSION=0.1.1
release:
	@test -n "$(VERSION)" || { echo "usage: make release VERSION=x.y.z" >&2; exit 2; }
	@test -z "$$(git status --porcelain)" || { echo "the tree is dirty" >&2; exit 2; }
	@git rev-parse -q --verify "refs/tags/v$(VERSION)" >/dev/null && \
	    { echo "v$(VERSION) already exists" >&2; exit 2; } || true
	sed -i '0,/^version = ".*"/s//version = "$(VERSION)"/' Cargo.toml
	printf '%s\n\n  * \n\n -- %s  %s\n\n%s' \
	    'smartclockmon ($(VERSION)-1) unstable; urgency=low' \
	    'Christopher Hoover <ch@murgatroid.com>' \
	    "$$(date -R)" \
	    "$$(cat packaging/debian/changelog)" > packaging/debian/changelog.new
	mv packaging/debian/changelog.new packaging/debian/changelog
	$$EDITOR packaging/debian/changelog
	$(CARGO) build --workspace
	git add Cargo.toml Cargo.lock packaging/debian/changelog
	git commit -m "Release $(VERSION)"
	git tag -a "v$(VERSION)" -m "Release $(VERSION)"
	@echo
	@echo "Tagged v$(VERSION).  Push it to build and publish:"
	@echo "    git push origin main && git push origin v$(VERSION)"

install-service:
	install -m 0644 packaging/systemd/smartclockd.service /etc/systemd/system/
	install -m 0644 -b packaging/systemd/smartclockd.default /etc/default/smartclockd
	systemctl daemon-reload
