# COPR invokes this as: make -f .copr/Makefile srpm outdir=<dir>
#
# IMPORTANT: COPR runs this inside MOCK, not a plain container, and mock redefines rpm's
# %{_topdir} to /builddir/build. An earlier version in retch called `rpmdev-setuptree` and then
# copied into $(HOME)/rpmbuild/SPECS -- which is where the tree lands in a plain container
# and is NOT where it lands under mock, so the copy failed with 'No such file or
# directory'. Verifying it in a container therefore proved nothing about COPR.
#
# This version does not depend on %{_topdir}, $(HOME), or a source tree existing at all:
# it points _sourcedir and _srcrpmdir explicitly, so the environment cannot move them.
#
# Why make_srpm rather than COPR's `rpkg` source type: this spec's Source0 is a remote tag
# tarball, and make_srpm makes the fetch explicit and reproducible.
#
# NOTE: this builds whatever version packaging/copr/rusticprofile.spec pins, which tracks the last
# RELEASED tag -- not the current Cargo.toml. That is deliberate; see NOTES.md.

SPEC    := packaging/copr/rusticprofile.spec
SOURCES := $(CURDIR)/.copr-sources

.PHONY: srpm

srpm:
	dnf -y install rpm-build rpmdevtools
	mkdir -p $(SOURCES) $(outdir)
	spectool -g -R --define '_sourcedir $(SOURCES)' $(SPEC)
	rpmbuild -bs --define '_sourcedir $(SOURCES)' --define '_srcrpmdir $(outdir)' $(SPEC)
