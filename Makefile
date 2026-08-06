.PHONY: test-agent

test-agent:
	@echo "Running agent tests..."
	@failed=0; \
	for f in tests/agent/*.sh; do \
	  echo "▶  $$f"; \
	  bash "$$f" && echo "  ✓ $$f" || { echo "  ✗ $$f"; failed=1; }; \
	done; \
	exit $$failed
