import assert from "node:assert/strict";
import {
    BuildString,
    PackageRecord,
    Version,
    VersionSpec,
} from "@conda-org/rattler";

if (!new VersionSpec("~=1.2.0").matches(new Version("1.2.3"))) {
    process.exit(1);
}

const record = new PackageRecord({
    name: "foo",
    version: "1.0",
    build: "0",
    build_number: 0,
    subdir: "noarch",
});
assert.throws(() => (record.build = ""), { code: "PARSE_BUILD_STRING" });
const build = BuildString.newUnchecked("");
record.build = build;
record.build = build;
assert.equal(record.build, "");
assert.equal(build.toString(), "");
