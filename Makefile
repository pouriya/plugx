TARGET_OPTION =

.PHONY: all build test clippy check-style docs open-docs examples

all: build clippy test check-style

build:
	cargo build --all-features

test:
	cargo test --all-features

clippy:
	cargo clippy --all-features --all-targets --no-deps -- -D warnings

check-style:
	cargo fmt --check --verbose

docs:
	cargo doc --all-features

open-docs:
	cargo doc --all-features --open

# Build every example in release mode and copy each artifact into bin/ under a
# sanitised snake-case name (hyphens become underscores; the words "example" and
# "plugx" are stripped out). Plugin examples are cdylibs, so their artifact is a
# lib*.so / lib*.dylib / *.dll rather than a binary.
examples:
	mkdir -p bin
	cargo build $(TARGET_OPTION) --release --all-features --examples
	@for src in examples/*.rs; do \
		[ -e "$$src" ] || continue; \
		name=$$(basename $$src .rs); \
		out=$$(echo $$name | tr 'A-Z-' 'a-z_' \
			| sed -e 's/example//g' -e 's/plugx//g' \
			      -e 's/__*/_/g' -e 's/^_//' -e 's/_$$//'); \
		for candidate in \
			target/release/examples/$$name \
			target/release/examples/lib$$name.so \
			target/release/examples/lib$$name.dylib \
			target/release/examples/$$name.dll; \
		do \
			[ -f "$$candidate" ] || continue; \
			suffix=$${candidate##*/}; \
			case $$suffix in \
				lib*.so)    target=bin/lib$$out.so ;; \
				lib*.dylib) target=bin/lib$$out.dylib ;; \
				*.dll)      target=bin/$$out.dll ;; \
				*)          target=bin/$$out ;; \
			esac; \
			echo ">>> $$name -> $$target"; \
			cp $$candidate $$target; \
		done; \
	done
