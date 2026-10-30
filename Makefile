# Wheel's tasks. Run from anywhere with `make -C <repo> <target>`; Wheel's runner buttons
# ([runner] in wheel.toml) call these. A client deploy never restarts the server (ADR-0020):
# deploy-client replaces the app and leaves wheeld running, restart-server is its own step.

ROOT := $(patsubst %/,%,$(dir $(abspath $(lastword $(MAKEFILE_LIST)))))
WHEEL_DERIVED_DATA ?= $(ROOT)/DerivedData
PROJECT_TOOLS := $(ROOT)/.claude/skills/project/tools

.PHONY: help build-server build-client dev test test-rust test-wheelkit test-query vet fmt kill \
	deploy-client deploy-dev restart-server restart-server-dev deploy-phone ios-check \
	grid-motion idle-chat lsregister-prune icon-render

help: ## List the targets
	@awk -F':.*## ' '/^[a-z-]+:.*## / { printf "  %-20s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

build-server: ## Build wheeld and the wheel CLI (cargo, debug)
	cd $(ROOT) && cargo build

build-client: ## Build the Debug Mac app into WHEEL_DERIVED_DATA (xcodegen, then xcodebuild)
	cd $(ROOT) && xcodegen generate --spec apple/project.yml --project apple
	xcodebuild -project $(ROOT)/apple/Wheel.xcodeproj -scheme Wheel -configuration Debug \
		-derivedDataPath "$(WHEEL_DERIVED_DATA)" build

dev: ## Run wheeld from source on its default port
	cd $(ROOT) && cargo run --bin wheeld

test: test-rust test-wheelkit test-query ## Run every test suite

test-rust: ## cargo test
	cd $(ROOT) && cargo test

test-wheelkit: ## swift test on WheelKit
	swift test --package-path $(ROOT)/apple/WheelKit

test-query: ## The beads artifact's query tests
	node $(ROOT)/artifacts/beads/query.test.js

vet: ## Clippy with warnings as errors
	cd $(ROOT) && cargo clippy --all-targets -- -D warnings

fmt: ## Format the Rust tree
	cd $(ROOT) && cargo fmt

kill: ## Stop a wheeld left running by `make dev`
	-pkill -f '$(ROOT)/target/debug/wheeld'

deploy-client: ## Build the Release app and install it as /Applications/Wheel.app; wheeld keeps running
	bash $(ROOT)/tools/install-app.sh release

deploy-dev: ## Build the dev app (DEV icon, .dev id, its own wheeld) and install it as /Applications/Wheel Dev.app
	bash $(ROOT)/tools/install-app.sh dev

restart-server: ## Restart Wheel.app's wheeld from its bundle; ends every terminal and agent session
	bash $(ROOT)/tools/restart-server.sh release

restart-server-dev: ## Restart Wheel Dev.app's wheeld from its bundle
	bash $(ROOT)/tools/restart-server.sh dev

deploy-phone: ## Build the phone app, install it on the device in WHEEL_IOS_DEVICE and launch it
	bash $(PROJECT_TOOLS)/deploy-phone.sh

ios-check: ## Build the phone app for the simulator and run the iOS check scripts against it
	bash $(PROJECT_TOOLS)/ios-checks.sh

grid-motion: ## Measure a divider drag and zoom toggles on a 2x2 terminal grid (needs make build-client)
	bash $(PROJECT_TOOLS)/grid-motion.sh

idle-chat: ## Check the installed Mac app and wheeld are quiet (run after a client deploy)
	bash $(PROJECT_TOOLS)/perf-chat.sh --live

lsregister-prune: ## Unregister Wheel.app copies outside /Applications (ARGS=--dry-run to list)
	bash $(PROJECT_TOOLS)/lsregister-prune.sh $(ARGS)

icon-render: ## Render apple/Mac/AppIcon.svg into the app icon's PNGs
	cd $(ROOT) && for s in 16 32 64 128 256 512 1024; do \
		png=apple/Mac/Assets.xcassets/AppIcon.appiconset/$$s.png; \
		resvg -w $$s -h $$s apple/Mac/AppIcon.svg $$png && magick $$png -alpha off $$png || exit 1; \
	done
