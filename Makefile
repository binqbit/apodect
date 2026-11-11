.PHONY: check check-ethereum check-solana check-near check-localnet fmt
.PHONY: local-up local-down local-status local-check local-smoke local-reset local-logs
.PHONY: check-crosschain check-apps

PYTHON_SOURCES = scripts/crosschain scripts/localnet.py scripts/tests contracts/ethereum/scripts
CROSSCHAIN_UV = UV_PROJECT_ENVIRONMENT="$(CURDIR)/.localnet/tools/crosschain" PYTHONDONTWRITEBYTECODE=1 uv run --project scripts/crosschain --locked

check: check-ethereum check-solana check-near check-localnet check-crosschain check-apps

check-apps:
	cargo fmt --manifest-path apps/Cargo.toml --all -- --check
	cargo clippy --manifest-path apps/Cargo.toml --workspace --locked --all-targets -- -D warnings
	cargo test --manifest-path apps/Cargo.toml --workspace --locked
	pnpm check
	pnpm build

check-ethereum:
	./contracts/ethereum/scripts/check.sh

check-solana:
	./contracts/solana/scripts/check.sh

check-near:
	./contracts/near/scripts/check.sh

fmt:
	cargo fmt --manifest-path apps/Cargo.toml --all
	pnpm format
	./contracts/ethereum/scripts/fmt.sh
	./contracts/solana/scripts/fmt.sh
	./contracts/near/scripts/fmt.sh
	$(CROSSCHAIN_UV) ruff check --config scripts/crosschain/pyproject.toml --select I --fix $(PYTHON_SOURCES)
	$(CROSSCHAIN_UV) ruff format --config scripts/crosschain/pyproject.toml $(PYTHON_SOURCES)

# One local stack: all three chains, contracts, relayer and simulated providers.
local-up local-down local-status local-check local-smoke local-logs:
	./scripts/localnet.sh $(patsubst local-%,%,$@)

local-reset:
	./scripts/localnet.sh reset $(if $(filter 1,$(CONFIRM)),--yes,)

check-localnet:
	PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -v

check-crosschain:
	$(CROSSCHAIN_UV) ruff check --config scripts/crosschain/pyproject.toml $(PYTHON_SOURCES)
	$(CROSSCHAIN_UV) ruff format --config scripts/crosschain/pyproject.toml --check $(PYTHON_SOURCES)
	$(CROSSCHAIN_UV) python -m unittest discover -s scripts/crosschain/tests -v
