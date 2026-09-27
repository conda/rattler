"""Rewrite the `Reference` nav entry in mkdocs.yml from TypeDoc's markdown output.

TypeDoc (with typedoc-plugin-markdown) has no native mkdocs/Zensical nav
integration, so this script walks the generated `docs/reference/` tree
(grouped into kind folders like `classes/`, `functions/`, `type-aliases/`)
and writes a matching nested `nav:` list into mkdocs.yml in place.
"""

from pathlib import Path

from ruamel.yaml import YAML

ROOT = Path(__file__).resolve().parent.parent.parent
REFERENCE_DIR = ROOT / "docs" / "reference"
MKDOCS_YML = ROOT / "mkdocs.yml"

# Preferred display order for TypeDoc's kind folders; anything else found on
# disk is appended afterwards, alphabetically.
KIND_ORDER = [
    "classes",
    "interfaces",
    "enums",
    "type-aliases",
    "variables",
    "functions",
]


def humanize(folder_name: str) -> str:
    return " ".join(word.capitalize() for word in folder_name.split("-"))


def build_reference_nav() -> list:
    nav: list = [{"Overview": "reference/index.md"}]

    kind_dirs = sorted(
        (p for p in REFERENCE_DIR.iterdir() if p.is_dir()),
        key=lambda p: (
            KIND_ORDER.index(p.name) if p.name in KIND_ORDER else len(KIND_ORDER),
            p.name,
        ),
    )

    for kind_dir in kind_dirs:
        pages = sorted(kind_dir.glob("*.md"), key=lambda p: p.stem)
        if not pages:
            continue
        entries = [
            {page.stem: f"reference/{kind_dir.name}/{page.name}"} for page in pages
        ]
        nav.append({humanize(kind_dir.name): entries})

    return nav


def main() -> None:
    if not REFERENCE_DIR.exists():
        raise SystemExit(
            f"{REFERENCE_DIR} does not exist — run `npx typedoc` first "
            "(the `_typedoc` pixi task does this automatically)."
        )

    yaml = YAML()
    yaml.preserve_quotes = True
    yaml.width = 4096

    config = yaml.load(MKDOCS_YML)

    for entry in config["nav"]:
        if isinstance(entry, dict) and "Reference" in entry:
            entry["Reference"] = build_reference_nav()
            break
    else:
        raise SystemExit("Could not find a 'Reference' entry in mkdocs.yml's nav")

    with MKDOCS_YML.open("w") as f:
        yaml.dump(config, f)


if __name__ == "__main__":
    main()
