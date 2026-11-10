# The GPUI client. Not part of the daemon's workspace or `make deploy-client`.
.PHONY: build test check run help

help:
	@echo "build  build the binaries, the Swift bridge and target/app/Wheel GPUI.app"
	@echo "test   unit tests (layout)"
	@echo "check  hidden regression check against a fixture wheeld (no window shown)"
	@echo "run    open the client on screen against a fixture wheeld"

build:
	python3 tools/build.py

test:
	cargo +stable test --manifest-path Cargo.toml --lib

check: build
	python3 tools/check.py

run: build
	python3 tools/run.py
