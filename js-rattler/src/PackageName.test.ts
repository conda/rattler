import { IsTrue, IsSame } from "./typeUtils";
import {
    PackageNameLiteral,
    isPackageName,
    normalizePackageName,
    isNormalizedPackageName,
} from "./PackageName";
import { expect, test } from "@jest/globals";

type Test1 = IsTrue<IsSame<PackageNameLiteral<"abc">, "abc">>;
type Test2 = IsTrue<IsSame<PackageNameLiteral<"invalid!">, never>>;
type Test3 = IsTrue<IsSame<PackageNameLiteral<"">, never>>;

test("isPackageName", () => {
    expect(isPackageName("abc")).toBeTruthy();
    expect(isPackageName("foo-bar")).toBeTruthy();
    expect(isPackageName("foo_bar")).toBeTruthy();
    expect(isPackageName("foo_bar.baz")).toBeTruthy();
    expect(isPackageName("Fo0_B4R-BaZ.B0b")).toBeTruthy();

    expect(isPackageName("")).toBeFalsy();
    expect(isPackageName("!")).toBeFalsy();
    expect(isPackageName(" ")).toBeFalsy();
    expect(isPackageName("$")).toBeFalsy();
});

test("isNormalizedPackageName", () => {
    expect(isNormalizedPackageName("abc")).toBeTruthy();
    expect(isNormalizedPackageName("foo-bar")).toBeTruthy();
    expect(isNormalizedPackageName("foo_bar")).toBeTruthy();
    expect(isNormalizedPackageName("foo_bar.baz")).toBeTruthy();

    expect(isNormalizedPackageName("Fo0_B4R-BaZ.B0b")).toBeFalsy();
    expect(isNormalizedPackageName("!")).toBeFalsy();
    expect(isNormalizedPackageName(" ")).toBeFalsy();
    expect(isNormalizedPackageName("$")).toBeFalsy();
    expect(isNormalizedPackageName("")).toBeFalsy();
});

test("normalizePackageName", () => {
    expect(normalizePackageName("abc")).toBe("abc");
    expect(normalizePackageName("aBc")).toBe("abc");
    expect(normalizePackageName("Fo0_B4R-BaZ.B0b")).toBe("fo0_b4r-baz.b0b");
});
