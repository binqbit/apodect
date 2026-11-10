.PHONY: check check-ethereum check-solana check-near fmt

check: check-ethereum check-solana check-near

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
