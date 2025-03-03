import { Platform } from "./Platform";
import { simple_solve as wasm_simple_solve, SolvedPackage } from "../pkg";

export { SolvedPackage } from "../pkg";

/**
 * Solve an environment
 *
 * @param specs - Matchspecs of packages that must be included.
 * @param channels - The channels to request for repodata of packages
 * @param platforms - The platforms to solve for
 * @public
 */
export async function simpleSolve(
    specs: string[],
    channels: string[],
    platforms: Platform[],
): Promise<SolvedPackage[]> {
    return await wasm_simple_solve(specs, channels, platforms);
}
