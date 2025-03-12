import { expect } from "@jest/globals";
import { Version } from "./Version";

function areVersionsEqual(a: unknown, b: unknown): boolean | undefined {
    const isAVersion = a instanceof Version;
    const isBVersion = b instanceof Version;

    if (isAVersion && isBVersion) {
        return a.equals(b);
    } else if (isAVersion === isBVersion) {
        return undefined;
    } else {
        return false;
    }
}

expect.addEqualityTesters([areVersionsEqual]);
