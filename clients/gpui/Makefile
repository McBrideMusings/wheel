# The GPUI client. Not part of the daemon's workspace or `make deploy-client`.
.PHONY: build build-dev test check run deploy deploy-dev compare compare-swift help

help:
	@echo "build       build the binaries, the Swift bridge and target/app/Wheel GPUI.app"
	@echo "build-dev   the same as target/app/Wheel GPUI Dev.app (DEV icon, .gpui.dev id, Wheel Dev's daemon)"
	@echo "test        unit tests (layout)"
	@echo "check       hidden regression check against a fixture wheeld (no window shown)"
	@echo "run         show the client against a fixture wheeld"
	@echo "deploy      build and install /Applications/Wheel GPUI.app; wheeld keeps running"
	@echo "deploy-dev  build and install /Applications/Wheel GPUI Dev.app beside it"
	@echo "compare     both clients hidden on one fixture wheeld: startup, memory, CPU, frames (results in /private/tmp/claude/wheel/compare)"
	@echo "compare-swift  the Swift app compare measures: its Debug configuration (debug server) built optimized"

build:
	python3 tools/build.py release

build-dev:
	python3 tools/build.py dev

test:
	cargo +stable test --manifest-path Cargo.toml --lib

check: build
	python3 tools/check.py

run: build
	python3 tools/run.py

deploy:
	bash tools/install.sh release

deploy-dev:
	bash tools/install.sh dev

REPO := $(abspath ../..)

compare-swift:
	xcodebuild -project $(REPO)/apple/Wheel.xcodeproj -scheme Wheel -configuration Debug \
		-derivedDataPath $(REPO)/DerivedData-compare-opt.noindex COMPILATION_CACHING=YES \
		SWIFT_OPTIMIZATION_LEVEL=-O SWIFT_COMPILATION_MODE=wholemodule GCC_OPTIMIZATION_LEVEL=s ENABLE_DEBUG_DYLIB=NO ENABLE_TESTABILITY=NO ONLY_ACTIVE_ARCH=YES \
		CODE_SIGN_IDENTITY="$$($(REPO)/tools/signing-identity)" CODE_SIGN_STYLE=Manual PROVISIONING_PROFILE_SPECIFIER= build

compare: build compare-swift
	python3 tools/compare.py
