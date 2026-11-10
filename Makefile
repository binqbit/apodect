.PHONY: check check-ethereum check-solana check-near check-localnet fmt
.PHONY: local-up local-down local-status local-check local-smoke local-reset local-logs

CHAIN ?= all

check: check-ethereum check-solana check-near check-localnet

check-ethereum:
	./contracts/ethereum/scripts/check.sh

check-solana:
	./contracts/solana/scripts/check.sh

check-near:
	./contracts/near/scripts/check.sh

fmt:
	./contracts/ethereum/scripts/fmt.sh
	./contracts/solana/scripts/fmt.sh
	./contracts/near/scripts/fmt.sh

# Persistent development chains. Select one with CHAIN=ethereum|solana|near.
local-up local-down local-status local-check local-smoke local-logs:
	python3 scripts/localnet.py $(patsubst local-%,%,$@) $(CHAIN)

# Reset is explicit and removes the selected chains, deployments and local keys.
local-reset:
	python3 scripts/localnet.py reset $(CHAIN) $(if $(filter 1,$(CONFIRM)),--yes,)

check-localnet:
	PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/tests -v
