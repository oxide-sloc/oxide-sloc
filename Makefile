.PHONY: help dev check fmt lint test build serve analyze bundle docker-build docker-run clean ci-lint jenkins-tests

help:
	@echo ""
	@echo "  oxide-sloc — available targets"
	@echo ""
	@echo "  Development"
	@echo "    make check          fmt + lint + test (no serve)"
	@echo "    make dev            fmt + lint + test + serve"
	@echo "    make fmt            cargo fmt --all"
	@echo "    make lint           cargo clippy -D warnings"
	@echo "    make test           cargo test --workspace"
	@echo "    make jenkins-tests  run the Jenkins CI guard suites (pure Python/bash, no Rust)"
	@echo "    make serve          start web UI on http://127.0.0.1:4317"
	@echo "    make analyze DIR=.  analyze a directory from the CLI"
	@echo ""
	@echo "  Build & Package"
	@echo "    make build          release binary → target/release/oxide-sloc"
	@echo "    make bundle         create transferable oxide-sloc-bundle.tar.gz (excludes target/ and .git/)"
	@echo "    make clean          cargo clean"
	@echo ""
	@echo "  Docker"
	@echo "    make docker-build   build Docker image"
	@echo "    make docker-run     run web UI in Docker on port 4317"
	@echo ""

# Run the full dev cycle: format, lint, test, then serve
dev: check serve

# All CI gates — run this before pushing
check: fmt lint test

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
	cargo test --workspace

build:
	cargo build --release -p oxide-sloc

serve:
	cargo run -p oxide-sloc -- serve

# Usage: make analyze DIR=./my-repo
analyze:
	cargo run -p oxide-sloc -- analyze $(DIR) --plain

bundle:
	@echo "Creating oxide-sloc-bundle.tar.gz (excludes target/, .git/, uncompressed vendor/) ..."
	tar --exclude=./target \
	    --exclude=./.git \
	    --exclude=./'*.tmp' \
	    --exclude=./out \
	    --exclude=./vendor \
	    --exclude=./oxide-sloc-bundle.tar.gz \
	    -czf oxide-sloc-bundle.tar.gz .
	@echo "Done: oxide-sloc-bundle.tar.gz  (vendor.tar.gz.* included; vendor/ uncompressed excluded)"

docker-build:
	docker build -t oxide-sloc .

docker-run:
	docker run --rm -p 4317:4317 oxide-sloc

clean:
	cargo clean

ci-lint:
	python3 -c "import xml.etree.ElementTree as ET; ET.parse('ci/jenkins/job-config.xml')"

# Jenkins CI guard suites — mirrors the ci.yml "pipeline-sim" job. Pure Python/bash,
# no Rust build, runs in seconds. Includes the REPO_URL air-gap resilience guards.
jenkins-tests:
	python3 ci/jenkins/tests/simulate-scenarios.py
	python3 ci/jenkins/tests/test-notify-confluence.py
	python3 ci/jenkins/tests/test-pipeline-helpers-guards.py
	python3 ci/jenkins/tests/test-repo-url-airgap.py
	python3 ci/jenkins/tests/test-windows-agent-dispatch.py
	python3 ci/jenkins/tests/test-bundle-artifacts.py
	bash   ci/jenkins/tests/test-notify-bitbucket.sh
	bash   ci/jenkins/tests/test-artifact-push.sh
