import { isPlatform, isArch, platformArch, platformNames } from "./Platform";
import { expect, test } from "@jest/globals";

test("isPlatform", () => {
    expect(isPlatform("linux-64")).toBeTruthy();
    expect(isPlatform("emscripten-wasm32")).toBeTruthy();
    expect(isPlatform("emscripten-wasm64")).toBeTruthy();

    expect(isPlatform("not-a-platform")).toBeFalsy();
    expect(isPlatform(42)).toBeFalsy();
});

test("isArch", () => {
    expect(isArch("x86_64")).toBeTruthy();
    expect(isArch("wasm32")).toBeTruthy();
    expect(isArch("wasm64")).toBeTruthy();

    expect(isArch("not-an-arch")).toBeFalsy();
});

test("platformArch", () => {
    expect(platformArch("linux-64")).toBe("x86_64");
    expect(platformArch("emscripten-wasm32")).toBe("wasm32");
    expect(platformArch("emscripten-wasm64")).toBe("wasm64");
    expect(platformArch("noarch")).toBeNull();
});

test("platformNames contains only strings", () => {
    for (const name of platformNames) {
        expect(typeof name).toBe("string");
    }
});
