import { simpleSolve } from "@conda-org/rattler";

const solved = await simpleSolve(
    ["python=3.11"],
    ["conda-forge"],
    ["linux-64"],
    undefined,
);

for (const pkg of solved) {
    console.log(`${pkg.packageName} ${pkg.version}`);
}
