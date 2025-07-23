import { Version, VersionSpec } from "@conda-org/rattler";

if (!new VersionSpec("~=1.2.0").matches(new Version("1.2.3"))) {
    process.exit(1);
}
