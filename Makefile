.PHONY: test

# Runs the unit tests and writes the LCOV report (lcov.info) that Harness Code Coverage uploads.
test:
	cargo llvm-cov --all-features --lcov --output-path lcov.info
