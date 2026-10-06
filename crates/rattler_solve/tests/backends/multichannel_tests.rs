//! Multichannels: channels that share a single channel priority tier, see
//! [`rattler_solve::ChannelRepoData`].
//!
//! Unless a test says otherwise, the expected selections match conda 26.7.3
//! with libmamba 2.9.0 for the same channels.

use super::helpers::{PackageBuilder, SolverCase, run_solver_cases};
use rattler_conda_types::RepoDataRecord;
use rattler_solve::{ChannelPriority, SolverImpl};

/// A record of `name` at `version` from the channel called `channel`.
fn package(name: &str, version: &str, channel: &str) -> RepoDataRecord {
    PackageBuilder::new(name)
        .version(version)
        .channel(&format!("https://conda.anaconda.org/{channel}/"))
        .build()
}

/// The members of a multichannel share a priority tier: strict priority
/// falls back to later members but never to channels outside the
/// multichannel, and flexible priority prefers the multichannel but picks
/// the highest version within it.
pub(super) fn multichannel_channel_priority<T: SolverImpl + Default>() {
    let pkg_a = package("pkg", "1.0", "chan-a");
    let pkg_b = package("pkg", "2.0", "chan-b");
    let pkg_c = package("pkg", "3.0", "chan-c");
    let case = |name| {
        SolverCase::new(name)
            .multi_channel_repository("grp", [pkg_a.clone()])
            .multi_channel_repository("grp", [pkg_b.clone()])
            .repository([pkg_c.clone()])
    };

    run_solver_cases::<T>(&[
        case("strict priority picks the highest version within the multichannel")
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg"])
            .expect_present([&pkg_b]),
        case("strict priority falls back to a later member")
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg>=1.5"])
            .expect_present([&pkg_b]),
        case("strict priority does not fall back to a channel after the multichannel")
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg>=2.5"])
            .expect_unsolvable(),
        case("flexible priority picks the highest version within the multichannel")
            .channel_priority(ChannelPriority::Flexible)
            .specs(["pkg"])
            .expect_present([&pkg_b]),
        case("flexible priority falls back to a channel after the multichannel")
            .channel_priority(ChannelPriority::Flexible)
            .specs(["pkg>=2.5"])
            .expect_present([&pkg_c]),
        case("disabled priority picks the highest version of any channel")
            .channel_priority(ChannelPriority::Disabled)
            .specs(["pkg"])
            .expect_present([&pkg_c]),
    ]);
}

/// Strict priority only restricts a package to the tier it is first found
/// in, so a package that no member provides still comes from a later channel.
pub(super) fn multichannel_strict_priority_is_per_package<T: SolverImpl + Default>() {
    let pkg_a = package("pkg", "1.0", "chan-a");
    let other_c = package("other", "1.0", "chan-c");

    SolverCase::new("a package missing from every member comes from a later channel")
        .multi_channel_repository("grp", [pkg_a.clone()])
        .repository([other_c.clone()])
        .channel_priority(ChannelPriority::Strict)
        .specs(["pkg", "other"])
        .expect_present([&pkg_a, &other_c])
        .run::<T>();
}

/// A multichannel after another channel forms one lower tier.
pub(super) fn multichannel_after_channel<T: SolverImpl + Default>() {
    let pkg_c = package("pkg", "3.0", "chan-c");
    let pkg_a = package("pkg", "1.0", "chan-a");
    let pkg_b = package("pkg", "2.0", "chan-b");
    let case = |name| {
        SolverCase::new(name)
            .repository([pkg_c.clone()])
            .multi_channel_repository("grp", [pkg_a.clone()])
            .multi_channel_repository("grp", [pkg_b.clone()])
            .specs(["pkg<3"])
    };

    run_solver_cases::<T>(&[
        case("strict priority excludes a multichannel after the first channel")
            .channel_priority(ChannelPriority::Strict)
            .expect_unsolvable(),
        case("flexible priority picks the highest version within the later multichannel")
            .channel_priority(ChannelPriority::Flexible)
            .expect_present([&pkg_b]),
    ]);
}

/// Every multichannel is its own tier.
pub(super) fn multichannel_multiple_multichannels<T: SolverImpl + Default>() {
    let pkg_a = package("pkg", "1.0", "chan-a");
    let pkg_b = package("pkg", "2.0", "chan-b");
    let pkg_c = package("pkg", "3.0", "chan-c");
    let pkg_d = package("pkg", "4.0", "chan-d");
    let case = |name| {
        SolverCase::new(name)
            .multi_channel_repository("first", [pkg_a.clone()])
            .multi_channel_repository("first", [pkg_b.clone()])
            .multi_channel_repository("second", [pkg_c.clone()])
            .multi_channel_repository("second", [pkg_d.clone()])
    };

    run_solver_cases::<T>(&[
        case("strict priority picks from the first multichannel")
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg"])
            .expect_present([&pkg_b]),
        case("strict priority does not fall back to the second multichannel")
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg>=2.5"])
            .expect_unsolvable(),
        case("flexible priority picks the highest version within the second multichannel")
            .channel_priority(ChannelPriority::Flexible)
            .specs(["pkg>=2.5"])
            .expect_present([&pkg_d]),
    ]);
}

/// Members share a tier even when another channel is listed between them.
/// conda cannot express this, because it always lists the members of a
/// multichannel next to each other.
pub(super) fn multichannel_members_apart<T: SolverImpl + Default>() {
    let pkg_a = package("pkg", "1.0", "chan-a");
    let pkg_c = package("pkg", "3.0", "chan-c");
    let pkg_b = package("pkg", "2.0", "chan-b");

    SolverCase::new("a member after another channel keeps the tier of the multichannel")
        .multi_channel_repository("grp", [pkg_a])
        .repository([pkg_c])
        .multi_channel_repository("grp", [pkg_b.clone()])
        .channel_priority(ChannelPriority::Strict)
        .specs(["pkg"])
        .expect_present([&pkg_b])
        .run::<T>();
}

/// With strict priority a dependency can come from another member than the
/// package that requires it.
pub(super) fn multichannel_dependency_from_other_member<T: SolverImpl + Default>() {
    let app_a = PackageBuilder::new("app")
        .version("1.0")
        .channel("https://conda.anaconda.org/chan-a/")
        .depends(["lib>=2"])
        .build();
    let lib_a = package("lib", "1.0", "chan-a");
    let lib_b = package("lib", "2.0", "chan-b");

    SolverCase::new("a dependency comes from a later member")
        .multi_channel_repository("grp", [app_a.clone(), lib_a])
        .multi_channel_repository("grp", [lib_b.clone()])
        .channel_priority(ChannelPriority::Strict)
        .specs(["app"])
        .expect_present([&app_a, &lib_b])
        .run::<T>();
}

/// The order of the members only breaks ties between identical candidates.
///
/// Within a multichannel candidates are ordered like across channels without
/// channel priority, so a higher build number wins over member order. conda
/// prefers the earlier channel over a higher build number in both situations.
pub(super) fn multichannel_ties<T: SolverImpl + Default>() {
    let pkg_b = package("pkg", "2.0", "chan-b");
    let pkg_d = package("pkg", "2.0", "chan-d");
    let pkg_d_rebuilt = PackageBuilder::new("pkg")
        .version("2.0")
        .build_number(1)
        .channel("https://conda.anaconda.org/chan-d/")
        .build();

    run_solver_cases::<T>(&[
        SolverCase::new("an identical candidate comes from the first member")
            .multi_channel_repository("grp", [pkg_b.clone()])
            .multi_channel_repository("grp", [pkg_d.clone()])
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg"])
            .expect_present([&pkg_b])
            .expect_absent([&pkg_d]),
        SolverCase::new("an identical candidate comes from the first member in reversed order")
            .multi_channel_repository("grp", [pkg_d.clone()])
            .multi_channel_repository("grp", [pkg_b.clone()])
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg"])
            .expect_present([&pkg_d])
            .expect_absent([&pkg_b]),
        SolverCase::new("a higher build number in a later member wins")
            .multi_channel_repository("grp", [pkg_b.clone()])
            .multi_channel_repository("grp", [pkg_d_rebuilt.clone()])
            .channel_priority(ChannelPriority::Strict)
            .specs(["pkg"])
            .expect_present([&pkg_d_rebuilt]),
    ]);
}

/// A channel-specific spec that names a multichannel accepts every member,
/// while a spec that names a member only accepts that member.
///
/// The first two cases match conda. Unlike conda, a member name only accepts
/// that member. Like any channel-specific spec in rattler, a named channel is
/// also selected when strict channel priority would exclude it, which conda
/// reports as unsolvable.
///
/// Only backends that support channel-specific specs run these cases.
pub(super) fn multichannel_channel_specific_specs<T: SolverImpl + Default>() {
    let pkg_a = package("pkg", "1.0", "chan-a");
    let pkg_b = package("pkg", "2.0", "chan-b");
    let pkg_c = package("pkg", "3.0", "chan-c");
    let case = |name| {
        SolverCase::new(name)
            .multi_channel_repository("grp", [pkg_a.clone()])
            .multi_channel_repository("grp", [pkg_b.clone()])
            .repository([pkg_c.clone()])
            .channel_priority(ChannelPriority::Strict)
    };

    run_solver_cases::<T>(&[
        case("the multichannel name selects from all members")
            .specs(["grp::pkg"])
            .expect_present([&pkg_b]),
        case("the multichannel name excludes channels outside the multichannel")
            .specs(["grp::pkg>=2.5"])
            .expect_unsolvable(),
        case("a member name selects only that member")
            .specs(["chan-a::pkg"])
            .expect_present([&pkg_a]),
        case("a member name excludes the other members")
            .specs(["chan-a::pkg>=1.5"])
            .expect_unsolvable(),
        case("a channel outside the multichannel can still be selected by name")
            .specs(["chan-c::pkg"])
            .expect_present([&pkg_c]),
        SolverCase::new("the multichannel name selects a multichannel after another channel")
            .repository([pkg_c.clone()])
            .multi_channel_repository("grp", [pkg_a.clone()])
            .multi_channel_repository("grp", [pkg_b.clone()])
            .channel_priority(ChannelPriority::Strict)
            .specs(["grp::pkg"])
            .expect_present([&pkg_b]),
    ]);
}
