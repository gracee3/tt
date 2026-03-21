APP_NAME := orcas
VERSION ?= 0.1.0
TARGET ?= x86_64-unknown-linux-gnu

PREFIX ?= /usr/local
BINDIR := $(PREFIX)/bin
LIBEXECDIR := $(PREFIX)/libexec/$(APP_NAME)
SHAREDIR := $(PREFIX)/share/$(APP_NAME)

SYSTEMD_DIR ?= $(HOME)/.config/systemd/user

DIST_NAME := $(APP_NAME)-v$(VERSION)-$(TARGET)
DIST_DIR := dist/$(DIST_NAME)

CARGO := cargo
E2E_RUNNER := tests/e2e/run_all.sh
SCENARIO ?=
TAG ?=

MAIN_BIN := orcas
AUX_BINS := orcasd orcas-tui
ALL_BINS := $(MAIN_BIN) $(AUX_BINS)

RELEASE_DIR := target/$(TARGET)/release

.PHONY: all
all: build

.PHONY: fmt
fmt:
	$(CARGO) fmt --all

.PHONY: clippy
clippy:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

.PHONY: test
test:
	$(CARGO) test --workspace

.PHONY: test-e2e
test-e2e:
	@E2E_SUITE=deterministic $(if $(SCENARIO),E2E_SCENARIO=$(SCENARIO),) $(if $(TAG),E2E_TAG=$(TAG),) ./$(E2E_RUNNER)

.PHONY: test-e2e-live
test-e2e-live:
	@E2E_SUITE=live $(if $(SCENARIO),E2E_SCENARIO=$(SCENARIO),) $(if $(TAG),E2E_TAG=$(TAG),) ./$(E2E_RUNNER)

.PHONY: test-e2e-long
test-e2e-long:
	@E2E_SUITE=long $(if $(SCENARIO),E2E_SCENARIO=$(SCENARIO),) $(if $(TAG),E2E_TAG=$(TAG),) ./$(E2E_RUNNER)

.PHONY: build
build:
	$(CARGO) build --workspace --release --target $(TARGET)

.PHONY: check
check:
	$(CARGO) check --workspace

.PHONY: doc
doc:
	$(CARGO) doc --workspace --no-deps

.PHONY: install
install: build
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 0755 "$(RELEASE_DIR)/$(MAIN_BIN)" "$(DESTDIR)$(BINDIR)/$(MAIN_BIN)"
	install -m 0755 "$(RELEASE_DIR)/orcasd" "$(DESTDIR)$(BINDIR)/orcasd"
	install -m 0755 "$(RELEASE_DIR)/orcas-tui" "$(DESTDIR)$(BINDIR)/orcas-tui"

.PHONY: install-user
install-user:
	$(MAKE) install PREFIX="$(HOME)/.local"

.PHONY: install-systemd
install-systemd:
	install -d "$(DESTDIR)$(SYSTEMD_DIR)"
	sed 's|^ExecStart=.*|ExecStart=$(BINDIR)/orcasd|' \
		packaging/systemd/orcas-daemon.service \
		> "$(DESTDIR)$(SYSTEMD_DIR)/orcas-daemon.service"
	chmod 0644 "$(DESTDIR)$(SYSTEMD_DIR)/orcas-daemon.service"

.PHONY: enable-systemd
enable-systemd:
	systemctl --user daemon-reload
	systemctl --user enable --now orcas-daemon.service

.PHONY: disable-systemd
disable-systemd:
	systemctl --user disable --now orcas-daemon.service || true

.PHONY: uninstall
uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/orcas"
	rm -f "$(DESTDIR)$(BINDIR)/orcasd"
	rm -f "$(DESTDIR)$(BINDIR)/orcas-tui"

.PHONY: uninstall-systemd
uninstall-systemd:
	rm -f "$(DESTDIR)$(SYSTEMD_DIR)/orcas-daemon.service"

.PHONY: dist
dist: build
	rm -rf "$(DIST_DIR)"
	install -d "$(DIST_DIR)/bin"
	install -d "$(DIST_DIR)/packaging/systemd"
	install -m 0755 "$(RELEASE_DIR)/orcas" "$(DIST_DIR)/bin/orcas"
	install -m 0755 "$(RELEASE_DIR)/orcasd" "$(DIST_DIR)/bin/orcasd"
	install -m 0755 "$(RELEASE_DIR)/orcas-tui" "$(DIST_DIR)/bin/orcas-tui"
	install -m 0644 packaging/systemd/orcas-daemon.service \
		"$(DIST_DIR)/packaging/systemd/orcas-daemon.service"
	test ! -f README.md || install -m 0644 README.md "$(DIST_DIR)/README.md"
	test ! -f LICENSE || install -m 0644 LICENSE "$(DIST_DIR)/LICENSE"
	cd dist && tar -czf "$(DIST_NAME).tar.gz" "$(DIST_NAME)"

.PHONY: clean
clean:
	$(CARGO) clean

.PHONY: clean-e2e
clean-e2e:
	rm -rf target/e2e

.PHONY: clean-dist
clean-dist:
	rm -rf dist
