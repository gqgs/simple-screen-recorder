PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
DESTDIR ?=
CARGO ?= cargo
CARGO_TARGET_DIR ?= target
INSTALL ?= install

.PHONY: all build install

all: build

build:
	$(CARGO) build --release --locked --target-dir "$(CARGO_TARGET_DIR)"

# Build as your regular user with `make` before running `sudo make install`.
install:
	$(INSTALL) -d "$(DESTDIR)$(BINDIR)"
	$(INSTALL) -m 755 "$(CARGO_TARGET_DIR)/release/ssr" "$(DESTDIR)$(BINDIR)/ssr"
