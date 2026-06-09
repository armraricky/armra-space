.PHONY: dev build release bump-patch bump-minor bump-major

dev:
	npm run tauri:dev

build:
	npm run tauri:build

release: bump-patch build

bump-patch:
	bash scripts/bump.sh patch

bump-minor:
	bash scripts/bump.sh minor

bump-major:
	bash scripts/bump.sh major
