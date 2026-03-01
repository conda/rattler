import { describe, expect, it } from "@jest/globals";
import { BuildNumberSpec } from "./BuildNumberSpec";

describe("BuildNumberSpec", () => {
    describe("constructor", () => {
        it("should parse a >= constraint", () => {
            expect(() => new BuildNumberSpec(">=3")).not.toThrow();
        });

        it("should parse a == constraint", () => {
            expect(() => new BuildNumberSpec("==7")).not.toThrow();
        });

        it("should parse a != constraint", () => {
            expect(() => new BuildNumberSpec("!=0")).not.toThrow();
        });

        it("should throw on invalid input", () => {
            expect(() => new BuildNumberSpec("notanumber")).toThrow();
        });
    });

    describe("toString", () => {
        it("should round-trip >= constraint", () => {
            expect(new BuildNumberSpec(">=3").toString()).toBe(">=3");
        });

        it("should round-trip == constraint", () => {
            expect(new BuildNumberSpec("==7").toString()).toBe("==7");
        });

        it("should round-trip != constraint", () => {
            expect(new BuildNumberSpec("!=0").toString()).toBe("!=0");
        });
    });

    describe("matches", () => {
        it(">= should match values at and above the threshold", () => {
            const spec = new BuildNumberSpec(">=3");
            expect(spec.matches(3)).toBe(true);
            expect(spec.matches(5)).toBe(true);
            expect(spec.matches(2)).toBe(false);
        });

        it("> should match strictly above", () => {
            const spec = new BuildNumberSpec(">3");
            expect(spec.matches(4)).toBe(true);
            expect(spec.matches(3)).toBe(false);
        });

        it("<= should match values at and below the threshold", () => {
            const spec = new BuildNumberSpec("<=3");
            expect(spec.matches(3)).toBe(true);
            expect(spec.matches(4)).toBe(false);
        });

        it("< should match strictly below", () => {
            const spec = new BuildNumberSpec("<3");
            expect(spec.matches(2)).toBe(true);
            expect(spec.matches(3)).toBe(false);
        });

        it("== should match exactly", () => {
            const spec = new BuildNumberSpec("==7");
            expect(spec.matches(7)).toBe(true);
            expect(spec.matches(6)).toBe(false);
        });

        it("!= should match everything except the value", () => {
            const spec = new BuildNumberSpec("!=0");
            expect(spec.matches(1)).toBe(true);
            expect(spec.matches(0)).toBe(false);
        });
    });
});
