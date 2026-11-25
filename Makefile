# Wheel's tasks. Run from anywhere with `make -C <repo> <target>`; Wheel's runner buttons
# ([runner] in wheel.toml) call these. A client deploy never restarts the server (ADR-0020):
# deploy-client replaces the app and leaves wheeld running, restart-server is its own step.

ROOT := $(patsubst %/,%,$(dir $(abspath $(lastword $(MAKEFILE_LIST)))))
WHEEL_DERIVED_DATA ?= $(ROOT)/DerivedData
PROJECT_TOOLS := $(ROOT)/.claude/skills/project-verify/tools

.PHONY: help build-server build-client dev test test-rust test-fast test-wheelkit test-wheelui test-wheelluau test-scripts vet fmt kill \
	deploy-client deploy-dev restart-server restart-server-dev deploy-linux restart-server-linux deploy-phone ios-check \
	grid-motion idle-chat lsregister-prune icon-render

help: ## List the targets
	@awk -F':.*## ' '/^[a-z-]+:.*## / { printf "  %-20s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

build-server: ## Build wheeld and the wheel CLI (cargo, debug)
	cd $(ROOT) && cargo build

build-client: ## Build the Debug Mac app into WHEEL_DERIVED_DATA (xcodegen, then xcodebuild)
	cd $(ROOT) && xcodegen generate --spec apple/project.yml --project apple
	xcodebuild -project $(ROOT)/apple/Wheel.xcodeproj -scheme Wheel -configuration Debug \
		-derivedDataPath "$(WHEEL_DERIVED_DATA)" COMPILATION_CACHING=YES CODE_SIGN_IDENTITY="$$($(ROOT)/tools/signing-identity)" CODE_SIGN_STYLE=Manual PROVISIONING_PROFILE_SPECIFIER= build

dev: ## Run wheeld from source on its default port
	cd $(ROOT) && cargo run --bin wheeld

test: test-rust test-wheelkit test-wheelui test-wheelluau test-scripts ## Run every test suite

test-rust: ## Every Rust test through cargo-nextest (tools/nextest), then the doctests
	cd $(ROOT) && tools/nextest run
	cd $(ROOT) && cargo test --doc

test-fast: ## Rust unit tests only, no integration binaries (nextest's fast profile)
	cd $(ROOT) && tools/nextest run --profile fast

lint-wheelkit: ## SwiftLint over WheelKit: no force-unwrap, try!, as!, unchecked range or subscript (a trap ends the app)
	swiftlint lint --strict --config $(ROOT)/.swiftlint.yml --quiet $(ROOT)/apple/WheelKit/Sources

test-wheelkit: lint-wheelkit ## swift test on WheelKit, after the lint
	swift test --package-path $(ROOT)/apple/WheelKit

test-wheelui: ## swift test on WheelUI (builds WheelLuau's C++ and Textual)
	swift test --package-path $(ROOT)/apple/WheelUI

test-wheelluau: ## swift test on WheelLuau (the pane-script VM)
	swift test --package-path $(ROOT)/apple/WheelLuau

test-scripts: ## The shell tests that need no daemon or app
	bash $(ROOT)/artifacts/companion/sessions.test.sh

vet: ## Clippy with warnings as errors, and the workspace-hack current (cargo install --locked cargo-hakari)
	cd $(ROOT) && cargo clippy --all-targets -- -D warnings
	cd $(ROOT) && cargo hakari generate --diff && cargo hakari manage-deps --dry-run

fmt: ## Format the Rust tree
	cd $(ROOT) && cargo fmt

kill: ## Stop a wheeld left running by `make dev`
	-pkill -f '$(ROOT)/target/debug/wheeld'

deploy-client: ## Build the GPUI client and install it as /Applications/Wheel.app, then deploy-linux; wheeld keeps running
	bash $(ROOT)/tools/install-app.sh release
	bash $(ROOT)/tools/linux-server.sh deploy

deploy-dev: ## Build the dev GPUI client (DEV icon, .dev id, its own wheeld) and install it as /Applications/Wheel Dev.app
	bash $(ROOT)/tools/install-app.sh dev

restart-server: ## Restart Wheel.app's wheeld from its bundle, then restart-server-linux; ends every terminal and agent
	bash $(ROOT)/tools/restart-server.sh release

restart-server-dev: ## Restart Wheel Dev.app's wheeld from its bundle
	bash $(ROOT)/tools/restart-server.sh dev

deploy-linux: ## Cross-compile wheeld and stage it on the Linux host (WHEEL_LINUX_HOST) without running it
	bash $(ROOT)/tools/linux-server.sh deploy

restart-server-linux: ## Run the staged wheeld on the Linux host: its systemd --user unit restarts
	bash $(ROOT)/tools/linux-server.sh restart

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

icon-render: ## Render clients/gpui/AppIcon.svg into the app icon's PNGs
	cd $(ROOT) && for s in 16 32 64 128 256 512 1024; do \
		png=clients/gpui/Assets.xcassets/AppIcon.appiconset/$$s.png; \
		resvg -w $$s -h $$s clients/gpui/AppIcon.svg $$png && magick $$png -alpha off $$png || exit 1; \
	done
	bash $(ROOT)/tools/render-dev-icon.sh
