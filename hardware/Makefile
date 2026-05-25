.PHONY: all setup regen clean

# Regenerate the wiring diagram from wiring.yml.
# Outputs (committed): wiring.svg, wiring.png, wiring.html, wiring.bom.tsv

VENV := .venv
WIREVIZ := $(VENV)/bin/wireviz

all: wiring.svg

$(WIREVIZ):
	python3 -m venv $(VENV)
	$(VENV)/bin/pip install --quiet --upgrade pip
	$(VENV)/bin/pip install --quiet wireviz

setup: $(WIREVIZ)
	@echo "WireViz ready. graphviz must also be on PATH (brew install graphviz)."

wiring.svg wiring.png wiring.html wiring.bom.tsv: wiring.yml $(WIREVIZ)
	$(WIREVIZ) wiring.yml

regen: clean wiring.svg

clean:
	rm -f wiring.svg wiring.png wiring.html wiring.bom.tsv wiring.gv
