# ==============================================================================
# Chassis AI Microkernel - Automation & Deployment Readiness Makefile
# ==============================================================================

.PHONY: all build build-release run test lint audit ready clean help

# Colors for terminal output
GREEN  := \033[1;32m
YELLOW := \033[1;33m
CYAN   := \033[1;36m
RESET  := \033[0m

all: ready

help:
	@echo "$(CYAN)Chassis Build & Security Automation:$(RESET)"
	@echo "  $(GREEN)make build$(RESET)           Compile the workspace (debug profile)"
	@echo "  $(GREEN)make build-release$(RESET)   Compile the workspace with release optimizations"
	@echo "  $(GREEN)make run$(RESET)             Run the Chassis CLI binary"
	@echo "  $(GREEN)make test$(RESET)            Run all unit and integration tests across workspace"
	@echo "  $(GREEN)make lint$(RESET)            Run clippy static analysis with warnings as errors"
	@echo "  $(GREEN)make audit$(RESET)           Run RustSec vulnerability scanner (Snyk equivalent)"
	@echo "  $(GREEN)make ready$(RESET)           Run full Deployment Readiness pipeline (tests + linter + audit)"
	@echo "  $(GREEN)make clean$(RESET)           Remove compiled target artifacts"

## 1. Compilation
build:
	@echo "$(YELLOW)--> Compiling workspace (debug)...$(RESET)"
	cargo build --workspace

build-release:
	@echo "$(YELLOW)--> Compiling workspace (release)...$(RESET)"
	cargo build --workspace --release

## 2. Execution
run:
	@echo "$(YELLOW)--> Running Chassis CLI...$(RESET)"
	cargo run -p chassis-cli

## 3. Testing
test:
	@echo "$(YELLOW)--> Running all tests across workspace...$(RESET)"
	cargo test --workspace

test-e2e:
	@echo "$(YELLOW)--> Running Complete E2E Integration Suite...$(RESET)"
	cargo test --test e2e_chassis_complete -- --nocapture

test-stress:
	@echo "$(YELLOW)--> Running Microkernel Stress & Resilience Suite...$(RESET)"
	cargo test --test stress_tests -- --nocapture

## 4. Static Code Quality & Linter
lint:
	@echo "$(YELLOW)--> Running Clippy static analysis (-D warnings)...$(RESET)"
	cargo clippy --workspace --all-targets -- -D warnings
	@echo "$(YELLOW)--> Verifying formatting (rustfmt)...$(RESET)"
	cargo fmt --all -- --check

## 5. Security & Vulnerability Scan (Snyk equivalent)
audit:
	@echo "$(YELLOW)--> Scanning dependencies against RustSec Advisory CVE Database...$(RESET)"
	cargo audit

## 6. Deployment Readiness (Runs compilation, all tests, linter, and security audit)
ready:
	@printf "$(CYAN)============================================================$(RESET)\n"
	@printf "$(CYAN)  CHASSIS: RUNNING DEPLOYMENT READINESS GATEWAY             $(RESET)\n"
	@printf "$(CYAN)============================================================$(RESET)\n"
	@printf "$(YELLOW)[1/4] Checking workspace build...$(RESET)\n"
	@cargo check --workspace --all-targets
	@printf "$(YELLOW)[2/4] Executing test suite...$(RESET)\n"
	@cargo test --workspace
	@printf "$(YELLOW)[3/4] Running strict linter (Clippy)...$(RESET)\n"
	@cargo clippy --workspace --all-targets -- -D warnings
	@printf "$(YELLOW)[4/4] Running security vulnerability audit (RustSec/Snyk)...$(RESET)\n"
	@cargo audit
	@printf "$(GREEN)============================================================$(RESET)\n"
	@printf "$(GREEN)  [CHASSIS] DEPLOYMENT READINESS: ALL GATES GREEN           $(RESET)\n"
	@printf "$(GREEN)  - Code builds cleanly                                     $(RESET)\n"
	@printf "$(GREEN)  - All tests passed                                        $(RESET)\n"
	@printf "$(GREEN)  - Zero clippy linter warnings                             $(RESET)\n"
	@printf "$(GREEN)  - Zero known CVE vulnerabilities in dependencies          $(RESET)\n"
	@printf "$(GREEN)============================================================$(RESET)\n"

clean:
	@echo "$(YELLOW)--> Cleaning target artifacts...$(RESET)"
	cargo clean
