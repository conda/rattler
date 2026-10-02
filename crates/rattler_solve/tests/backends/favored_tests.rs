//! A locked ("favored") package must be kept whenever a solution keeps it,
//! regardless of spec order. Reproduces `pixi update cython` upgrading a
//! locked `python` 3.13 to 3.14 even though a `cython` build for 3.13 existed.

use super::helpers::{PackageBuilder, SolverCase, run_solver_cases};
use rattler_conda_types::RepoDataRecord;
use rattler_solve::SolverImpl;

/// `python` 3.13.11 and 3.14.7 with their `python_abi`, and `cython` 3.2.2
/// (py313) and 3.3.0 (py313 and py314). Returns the repository and the
/// python 3.13 records to lock.
fn favored_repo() -> (Vec<RepoDataRecord>, RepoDataRecord, RepoDataRecord) {
    let python_abi_313 = PackageBuilder::new("python_abi")
        .version("3.13")
        .build_string("5_cp313")
        .build();
    let python_abi_314 = PackageBuilder::new("python_abi")
        .version("3.14")
        .build_string("5_cp314")
        .build();

    let python_313 = PackageBuilder::new("python")
        .version("3.13.11")
        .build_string("hd6d6ee5_0_cp313")
        .depends(["python_abi 3.13.* *_cp313"])
        .build();
    let python_314 = PackageBuilder::new("python")
        .version("3.14.7")
        .build_string("hd6d6ee5_0_cp314")
        .depends(["python_abi 3.14.* *_cp314"])
        .build();

    let cython_322_py313 = PackageBuilder::new("cython")
        .version("3.2.2")
        .build_string("py313h5b4e0ec_0")
        .depends(["python >=3.13,<3.14.0a0", "python_abi 3.13.* *_cp313"])
        .build();
    let cython_330_py313 = PackageBuilder::new("cython")
        .version("3.3.0")
        .build_string("py313h5b4e0ec_0")
        .depends(["python >=3.13,<3.14.0a0", "python_abi 3.13.* *_cp313"])
        .build();
    let cython_330_py314 = PackageBuilder::new("cython")
        .version("3.3.0")
        .build_string("py314h5b4e0ec_0")
        .depends(["python >=3.14,<3.15.0a0", "python_abi 3.14.* *_cp314"])
        .build();

    let repo = vec![
        python_abi_313.clone(),
        python_abi_314,
        python_313.clone(),
        python_314,
        cython_322_py313,
        cython_330_py313,
        cython_330_py314,
    ];

    (repo, python_313, python_abi_313)
}

/// A locked `python` is kept with the matching `cython` build, whether
/// `cython` or `python` is decided first.
pub(super) fn favored_kept_independent_of_spec_order<T: SolverImpl + Default>() {
    let (repo, python_313, python_abi_313) = favored_repo();

    run_solver_cases::<T>(&[
        SolverCase::new("favored python survives cython-first spec order")
            .repository(repo.clone())
            .specs(["cython", "python"])
            .locked_packages([python_313.clone(), python_abi_313.clone()])
            .expect_present([("python", "3.13.11")])
            .expect_present([("cython", "3.3.0", "py313h5b4e0ec_0")])
            .expect_absent([("python", "3.14.7")])
            .expect_absent([("cython", "3.3.0", "py314h5b4e0ec_0")]),
        SolverCase::new("favored python survives python-first spec order")
            .repository(repo)
            .specs(["python", "cython"])
            .locked_packages([python_313, python_abi_313])
            .expect_present([("python", "3.13.11")])
            .expect_present([("cython", "3.3.0", "py313h5b4e0ec_0")])
            .expect_absent([("python", "3.14.7")])
            .expect_absent([("cython", "3.3.0", "py314h5b4e0ec_0")]),
    ]);
}

/// A locked `python` reachable only through the locked `foo` is kept.
pub(super) fn favored_kept_via_favored_parent<T: SolverImpl + Default>() {
    let (mut repo, python_313, python_abi_313) = favored_repo();

    let foo = PackageBuilder::new("foo")
        .version("1.0")
        .depends(["python"])
        .build();
    repo.push(foo.clone());

    SolverCase::new("favored python kept when reachable only through a favored parent")
        .repository(repo)
        .specs(["cython", "foo"])
        .locked_packages([python_313, python_abi_313, foo])
        .expect_present([("python", "3.13.11")])
        .expect_present([("cython", "3.3.0", "py313h5b4e0ec_0")])
        .expect_absent([("python", "3.14.7")])
        .expect_absent([("cython", "3.3.0", "py314h5b4e0ec_0")])
        .run::<T>();
}

/// A locked `python` reachable only through the unlocked `cython` spec is
/// kept.
pub(super) fn favored_kept_via_unlocked_target<T: SolverImpl + Default>() {
    let (repo, python_313, python_abi_313) = favored_repo();

    SolverCase::new("favored python kept when reachable only through the unlocked target")
        .repository(repo)
        .specs(["cython"])
        .locked_packages([python_313, python_abi_313])
        .expect_present([("python", "3.13.11")])
        .expect_present([("cython", "3.3.0", "py313h5b4e0ec_0")])
        .expect_absent([("python", "3.14.7")])
        .expect_absent([("cython", "3.3.0", "py314h5b4e0ec_0")])
        .run::<T>();
}
