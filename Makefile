# Taskologic build and install. `make setup` walks through the first install which means on a Linux host: group, daemon user, service, first user. 
# See docs/running.md for what this bs means.

PREFIX  ?= /usr/local
BINDIR   = $(PREFIX)/bin
UNITDIR ?= /etc/systemd/system
CARGO   ?= cargo
RESTART ?= yes

# Only the steps that write outside the tree are elevated, and only when they
# have to be. Building stays unprivileged on purpose: `sudo cargo build` uses
# root's CARGO_HOME, so every dependency fingerprint changes and the whole
# workspace rebuilds, then leaves root owned files in target/ that your next
# ordinary build cannot write. A staged install (DESTDIR, i.e. packaging)
# writes into a directory you already own, so it never needs sudo either.
ifeq ($(strip $(DESTDIR)),)
SUDO := $(shell if [ "$$(id -u)" = 0 ]; then echo; elif command -v sudo >/dev/null 2>&1; then echo sudo; fi)
else
SUDO :=
endif

.PHONY: all help build test install setup update db-status migrate uninstall

# Bare `make` builds, as it does everywhere else.
all: build

help:
	@echo "Taskologic targets:"
	@echo "  make            build release binaries (same as 'make build')"
	@echo "  make build      build release binaries"
	@echo "  make test       run the test suite"
	@echo "  make install    install binaries and the systemd unit (asks for sudo)"
	@echo "  make setup      interactive first time setup: group, daemon user, service, first user (asks for sudo)"
	@echo "  make update     upgrade an existing install in place, keeps config and data (asks for sudo)"
	@echo "  make db-status  show the database schema version and any pending migrations"
	@echo "  make migrate    back up the database and apply pending migrations (asks for sudo)"
	@echo "  make uninstall  remove binaries and the service, keeps config and data (asks for sudo)"
	@echo
	@echo "Do not put sudo in front of these yourself: they elevate the few steps"
	@echo "that need it and build as you, which keeps target/ yours and warm."

build:
	$(CARGO) build --release --workspace

test:
	$(CARGO) test --workspace

install: build
	@if [ "$$(id -u)" != 0 ] && [ -z "$(SUDO)" ] && [ -z "$(DESTDIR)" ]; then \
		echo "error: installing needs root and sudo is not available here; re-run as root" >&2; \
		exit 1; \
	fi
	$(SUDO) install -d $(DESTDIR)$(BINDIR)
	$(SUDO) install -m 755 target/release/taskologicd $(DESTDIR)$(BINDIR)/taskologicd
	$(SUDO) install -m 755 target/release/taskologic $(DESTDIR)$(BINDIR)/taskologic
	$(SUDO) install -d $(DESTDIR)$(UNITDIR)
	$(SUDO) install -m 644 packaging/taskologicd.service $(DESTDIR)$(UNITDIR)/taskologicd.service
	@echo "Installed. Run 'make setup' for the first time setup."

setup:
	@BINDIR=$(BINDIR) UNITDIR=$(UNITDIR) ./scripts/setup.sh

# Upgrade a host that `make setup` already set up: new binaries, new unit,
# daemon restarted. Config, database, users and groups are left as they are.
# RESTART=no installs without restarting the running daemon.
update:
	@BINDIR=$(BINDIR) UNITDIR=$(UNITDIR) RESTART=$(RESTART) ./scripts/update.sh

# The installed daemon answers both of these without starting up, so they are
# safe to run while it is serving. `migrate` is not: stop the daemon first, or
# it carries on against a schema that moved under it. `make update` does the
# stop, the migration and the restart in the right order, and is what you
# normally want; these two are for looking, and for fixing up by hand.
db-status:
	@test -x $(BINDIR)/taskologicd || { echo "error: no taskologicd at $(BINDIR), run 'make install' first" >&2; exit 1; }
	@$(BINDIR)/taskologicd --db-status

migrate:
	@test -x $(BINDIR)/taskologicd || { echo "error: no taskologicd at $(BINDIR), run 'make install' first" >&2; exit 1; }
	@echo "This wants the daemon stopped: sudo systemctl stop taskologicd"
	$(SUDO) $(BINDIR)/taskologicd --migrate

uninstall:
	-$(SUDO) systemctl disable --now taskologicd 2>/dev/null
	$(SUDO) rm -f $(DESTDIR)$(UNITDIR)/taskologicd.service
	-$(SUDO) systemctl daemon-reload 2>/dev/null
	$(SUDO) rm -f $(DESTDIR)$(BINDIR)/taskologicd $(DESTDIR)$(BINDIR)/taskologic
	@echo "Removed binaries and the service."
	@echo "Kept /etc/taskologic, /var/lib/taskologic, the taskologic group and the daemon user."
	@echo "If you want those gone too:"
	@echo "  userdel taskologic; groupdel taskologic; rm -rf /etc/taskologic /var/lib/taskologic"
