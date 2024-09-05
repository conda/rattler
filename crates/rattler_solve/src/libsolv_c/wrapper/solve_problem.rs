use super::ffi;
use super::solvable::SolvableId;

use ffi::{
    SolverRuleinfo_SOLVER_RULE_JOB as SOLVER_RULE_JOB,
    SolverRuleinfo_SOLVER_RULE_JOB_NOTHING_PROVIDES_DEP as SOLVER_RULE_JOB_NOTHING_PROVIDES_DEP,
    SolverRuleinfo_SOLVER_RULE_JOB_UNKNOWN_PACKAGE as SOLVER_RULE_JOB_UNKNOWN_PACKAGE,
    SolverRuleinfo_SOLVER_RULE_PKG as SOLVER_RULE_PKG,
    SolverRuleinfo_SOLVER_RULE_PKG_CONFLICTS as SOLVER_RULE_SOLVER_RULE_PKG_CONFLICTS,
    SolverRuleinfo_SOLVER_RULE_PKG_CONSTRAINS as SOLVER_RULE_PKG_CONSTRAINS,
    SolverRuleinfo_SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP as SOLVER_RULE_SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP,
    SolverRuleinfo_SOLVER_RULE_PKG_REQUIRES as SOLVER_RULE_PKG_REQUIRES,
    SolverRuleinfo_SOLVER_RULE_PKG_SAME_NAME as SOLVER_RULE_SOLVER_RULE_PKG_SAME_NAME,
    SolverRuleinfo_SOLVER_RULE_UPDATE as SOLVER_RULE_SOLVER_RULE_UPDATE,
};

#[derive(Debug)]
pub enum SolveProblem {
    /// A top level requirement.
    /// The difference between JOB and PKG is unknown (possibly unused).
    Job { dep: String },
    /// A top level dependency does not exist.
    /// Could be a wrong name or missing channel.
    JobNothingProvidesDep { dep: String },
    /// A top level dependency does not exist.
    /// Could be a wrong name or missing channel.
    JobUnknownPackage { dep: String },
    /// A top level requirement.
    /// The difference between JOB and PKG is unknown (possibly unused).
    Pkg { dep: String },
    /// Looking for a valid solution to the installation satisfiability expand to
    /// two solvables of same package that cannot be installed together. This is
    /// a partial explanation of why one of the solvables (could be any of the
    /// parent) cannot be installed.
    PkgConflicts {
        source: SolvableId,
        target: SolvableId,
    },
    /// A constraint (`run_constrained`) on source is conflicting with target.
    /// `SOLVER_RULE_PKG_CONSTRAINS` has a dep, but it can resolve to nothing.
    /// The constraint conflict is actually expressed between the target and
    /// a constrains node child of the source.
    PkgConstrains {
        source: SolvableId,
        target: SolvableId,
        dep: String,
    },
    /// A package dependency does not exist.
    /// Could be a wrong name or missing channel.
    /// This is a partial explanation of why a specific solvable (could be any
    /// of the parent) cannot be installed.
    PkgNothingProvidesDep { source: SolvableId, dep: String },
    /// Express a dependency on source that is involved in explaining the
    /// problem.
    /// Not all dependency of package will appear, only enough to explain the
    //. problem. It is not a problem in itself, only a part of the graph.
    PkgRequires { source: SolvableId, dep: String },
    /// Package conflict between two solvables of same package name (handled the same as
    /// [`SolveProblem::PkgConflicts`]).
    PkgSameName {
        source: SolvableId,
        target: SolvableId,
    },
    /// Encountered in the problems list from libsolv but unknown.
    /// Explicitly ignored until we do something with it.
    Update,
}

impl SolveProblem {
    pub fn from_raw(
        problem_type: ffi::SolverRuleinfo,
        dep: Option<String>,
        source: Option<SolvableId>,
        target: Option<SolvableId>,
    ) -> Self {
        match problem_type {
            SOLVER_RULE_JOB => Self::Job { dep: dep.unwrap() },
            SOLVER_RULE_JOB_NOTHING_PROVIDES_DEP => {
                Self::JobNothingProvidesDep { dep: dep.unwrap() }
            }
            SOLVER_RULE_JOB_UNKNOWN_PACKAGE => Self::JobUnknownPackage { dep: dep.unwrap() },
            SOLVER_RULE_PKG => Self::Pkg { dep: dep.unwrap() },
            SOLVER_RULE_SOLVER_RULE_PKG_CONFLICTS => Self::PkgConflicts {
                source: source.unwrap(),
                target: target.unwrap(),
            },
            SOLVER_RULE_PKG_CONSTRAINS => Self::PkgConstrains {
                source: source.unwrap(),
                target: target.unwrap(),
                dep: dep.unwrap(),
            },
            SOLVER_RULE_SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP => Self::PkgNothingProvidesDep {
                source: source.unwrap(),
                dep: dep.unwrap(),
            },
            SOLVER_RULE_PKG_REQUIRES => Self::PkgRequires {
                source: source.unwrap(),
                dep: dep.unwrap(),
            },
            SOLVER_RULE_SOLVER_RULE_PKG_SAME_NAME => Self::PkgSameName {
                source: source.unwrap(),
                target: target.unwrap(),
            },
            SOLVER_RULE_SOLVER_RULE_UPDATE => Self::Update,
            _ => panic!("Unknown problem type: {problem_type}"),
        }
    }
}
