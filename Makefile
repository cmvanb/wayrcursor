PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
UNITDIR ?= $(or $(XDG_DATA_HOME),$(HOME)/.local/share)/systemd/user
E2E_MODES := locate daemon shake drift reload

.PHONY: build install uninstall test e2e perf

build:
	cargo build --release --locked

install: build
	install -Dm755 target/release/wayrcursor $(DESTDIR)$(BINDIR)/wayrcursor
	sed 's|@BINDIR@|$(BINDIR)|' dist/wayrcursor.service.in > target/wayrcursor.service
	install -Dm644 target/wayrcursor.service $(DESTDIR)$(UNITDIR)/wayrcursor.service

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/wayrcursor $(DESTDIR)$(UNITDIR)/wayrcursor.service

test:
	cargo test --locked

e2e:
	@for mode in $(E2E_MODES); do \
		echo "== $$mode"; \
		MODE=$$mode scripts/e2e.sh target/e2e/$$mode | tail -1 | grep -qx PASS || { echo "$$mode failed: see target/e2e/$$mode"; exit 1; }; \
	done

perf:
	scripts/perf.sh
