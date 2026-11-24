# The GPUI client, its own workspace. The root `make deploy-client` and `make deploy-dev` install it
# as /Applications/Wheel.app and /Applications/Wheel Dev.app (ADR-0040).
.PHONY: build build-dev build-check fixture-binaries test test-ui check run compare compare-swift help

help:
	@echo "build       build the binaries, the Swift bridge and target/app/Wheel.app"
	@echo "build-dev   the same as target/app/Wheel Dev.app (DEV icon, .dev id, its own wheeld on 7422)"
	@echo "build-check build, then target/app/check/Wheel.app: the bundle plus the hidden check"
	@echo "test        unit tests of the GPUI-free logic (wheel-gpui-core), no GPUI build"
	@echo "test-ui     unit tests of the code that draws (wheel-gpui, builds GPUI)"
	@echo "fixture-binaries  build the programs the fixture wheeld runs"
	@echo "check       hidden regression check against a fixture wheeld (no window shown)"
	@echo "run         show the client against a fixture wheeld"
	@echo "compare     both clients hidden on one fixture wheeld: startup, memory, CPU, frames (results in /private/tmp/claude/wheel/compare)"
	@echo "compare-swift  the Swift app compare measures: its Debug configuration (debug server) built optimized"

build:
	python3 tools/build.py release

build-dev:
	python3 tools/build.py dev

build-check:
	python3 tools/build.py release --check

test:
	cargo +stable test --manifest-path Cargo.toml -p wheel-gpui-core

test-ui:
	cargo +stable test --manifest-path Cargo.toml --lib -p wheel-gpui

# The programs the fixture wheeld runs, built from the repo root so a stale build never fails the check.
fixture-binaries:
	python3 tools/fixture.py

check: build-check fixture-binaries
	python3 tools/check.py

run: build fixture-binaries
	python3 tools/run.py

REPO := $(abspath ../..)

compare-swift:
	xcodebuild -project $(REPO)/apple/Wheel.xcodeproj -scheme Wheel -configuration Debug \
		-derivedDataPath $(REPO)/DerivedData-compare-opt.noindex COMPILATION_CACHING=YES \
		SWIFT_OPTIMIZATION_LEVEL=-O SWIFT_COMPILATION_MODE=wholemodule GCC_OPTIMIZATION_LEVEL=s ENABLE_DEBUG_DYLIB=NO ENABLE_TESTABILITY=NO ONLY_ACTIVE_ARCH=YES \
		CODE_SIGN_IDENTITY="$$($(REPO)/tools/signing-identity)" CODE_SIGN_STYLE=Manual PROVISIONING_PROFILE_SPECIFIER= build

compare: build compare-swift
	python3 tools/compare.py
