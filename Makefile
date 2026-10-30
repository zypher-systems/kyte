# Thin wrappers around cargo (section 12). Docker targets use Docker, never Podman.
CARGO ?= cargo
DOCKER ?= docker
# On SELinux-enforcing hosts (Fedora) bind mounts need the :Z suffix.
# Never disable SELinux; relabel the mount instead.
MOUNT_SUFFIX ?= :Z

.PHONY: build test lint e2e demo docker-build docker-test clean

build:
	$(CARGO) build --release

test:
	$(CARGO) test

lint:
	$(CARGO) fmt --check
	$(CARGO) clippy --all-targets -- -D warnings

e2e:
	$(CARGO) test --test e2e

demo:
	./scripts/demo.sh

docker-build:
	$(DOCKER) build --target dev -t kyte-dev .

docker-test: docker-build
	$(DOCKER) run --rm -v "$(CURDIR)":/src$(MOUNT_SUFFIX) -w /src kyte-dev cargo test

clean:
	$(CARGO) clean
