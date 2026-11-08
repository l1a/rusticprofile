# COPR invokes this as: make -f .copr/Makefile srpm outdir=<dir>
#
# IMPORTANT: COPR runs this inside MOCK, not a plain container, and mock redefines rpm's
# %{_topdir} to /builddir/build. An earlier version in retch called `rpmdev-setuptree` and
# then copied into $(HOME)/rpmbuild/SPECS -- which is where the tree lands in a plain
# container and is NOT where it lands under mock, so the copy failed with 'No such file or
# directory'. Verifying it in a container therefore proved nothing about COPR.
#
# This version does not depend on %{_topdir}, $(HOME), or a source tree existing at all:
# it points _sourcedir and _srcrpmdir explicitly, so the environment cannot move them.
#
# packaging/copr/rusticprofile.spec is a TEMPLATE: its Version: is `@VERSION@` and its
# Source0 is a local tarball. This recipe supplies both, from the checkout COPR cloned:
#
#   version  <- Cargo.toml, via scripts/render_packaging.py
#   Source0  <- an archive of this checkout
#
# Neither needs a published tag or a network fetch, which lets a PR build its own SRPM in CI.

SPEC_IN := packaging/copr/rusticprofile.spec
BUILD   := $(CURDIR)/.copr-build
RENDER  := scripts/render_packaging.py

.PHONY: srpm

srpm:
	dnf -y install rpm-build python3 git
	rm -rf $(BUILD)
	mkdir -p $(BUILD) $(outdir)
	V=$$(python3 $(RENDER) --print-version); \
	echo "version from Cargo.toml: $$V"; \
	python3 $(RENDER) --target copr --version "$$V" --out $(BUILD)/rusticprofile.spec; \
	TARBALL=$(BUILD)/rusticprofile-$$V.tar.gz; \
	if git rev-parse --git-dir >/dev/null 2>&1; then \
	    echo "source: git archive HEAD (the committed tree)"; \
	    git archive --format=tar.gz --prefix="rusticprofile-$$V/" -o "$$TARBALL" HEAD; \
	else \
	    echo "source: tar of the working tree (no git repository here)"; \
	    tar --exclude=./.git --exclude=./target --exclude=./.copr-build \
	        --exclude=WIP.md --exclude=memory --exclude=.antigravitycli \
	        --exclude=settings.local.json --exclude=__pycache__ --exclude='*.pyc' \
	        --transform "s,^[.],rusticprofile-$$V," \
	        -czf "$$TARBALL" .; \
	fi; \
	tar -tzf "$$TARBALL" > $(BUILD)/listing.txt; \
	echo "packed $$(wc -l < $(BUILD)/listing.txt) entries as rusticprofile-$$V/"; \
	if grep -qE "^rusticprofile-[^/]+/(WIP[.]md|memory/|target/|[.]git/|[.]antigravitycli/|settings[.]local[.]json)" $(BUILD)/listing.txt; then \
	    echo "error: the source archive contains files that must never ship:" >&2; \
	    grep -E "^rusticprofile-[^/]+/(WIP[.]md|memory/|target/|[.]git/|[.]antigravitycli/|settings[.]local[.]json)" $(BUILD)/listing.txt >&2; \
	    exit 1; \
	fi; \
	for f in Cargo.toml Cargo.lock LICENSE docs/rusticprofile.1; do \
	    grep -qx "rusticprofile-$$V/$$f" $(BUILD)/listing.txt \
	        || { echo "error: the source archive is missing $$f" >&2; exit 1; }; \
	done; \
	echo "source archive verified"
	rpmbuild -bs --define '_sourcedir $(BUILD)' --define '_srcrpmdir $(outdir)' $(BUILD)/rusticprofile.spec
