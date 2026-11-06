/*
 * Smoke test of the installed C API. Build against the installed library with
 *
 *   cc tests/c/test_api.c $(pkg-config --cflags --libs rattler) -o test_api
 */
#include <rattler.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int failures = 0;

#define CHECK(cond)                                                       \
    do {                                                                  \
        if (!(cond)) {                                                    \
            fprintf(stderr, "%s:%d: check failed: %s\n", __FILE__,        \
                    __LINE__, #cond);                                     \
            failures++;                                                   \
        }                                                                 \
    } while (0)

static RattlerVersion *parse_version(const char *s) {
    RattlerVersion *version = NULL;
    RattlerStatus status = rattler_version_parse(s, strlen(s), &version);
    if (status != RATTLER_STATUS_OK) {
        RattlerStr err = rattler_take_last_error();
        fprintf(stderr, "failed to parse version '%s': %s\n", s, err.data);
        rattler_str_free(err);
        exit(1);
    }
    return version;
}

static int str_eq(RattlerStr s, const char *expected) {
    int eq = s.data != NULL && s.len == strlen(expected) &&
             memcmp(s.data, expected, s.len) == 0;
    rattler_str_free(s);
    return eq;
}

static void test_version(void) {
    RattlerVersion *a = parse_version("1.2.3");
    RattlerVersion *b = parse_version("1.10");
    RattlerVersion *prefix = parse_version("1.2");

    CHECK(str_eq(rattler_version_to_string(a), "1.2.3"));
    CHECK(rattler_version_compare(a, b) < 0);
    CHECK(rattler_version_compare(b, a) > 0);
    CHECK(rattler_version_starts_with(a, prefix));
    CHECK(rattler_version_segment_count(a) == 3);

    uint64_t major = 0, minor = 0;
    CHECK(rattler_version_as_major_minor(a, &major, &minor));
    CHECK(major == 1 && minor == 2);

    rattler_version_free(a);
    rattler_version_free(b);
    rattler_version_free(prefix);
}

static void test_version_error(void) {
    const char *s = "1.2..3";
    RattlerVersion *version = NULL;
    CHECK(rattler_version_parse(s, strlen(s), &version) ==
          RATTLER_STATUS_PARSE_ERROR);
    CHECK(version == NULL);

    RattlerStr err = rattler_take_last_error();
    CHECK(err.data != NULL && err.len > 0);
    rattler_str_free(err);
}

static void test_match_spec(void) {
    const char *s = "conda-forge::numpy >=1.20,<2 py312*";
    RattlerMatchSpec *spec = NULL;
    CHECK(rattler_match_spec_parse(s, strlen(s), RATTLER_PARSE_STRICTNESS_STRICT,
                                   &spec) == RATTLER_STATUS_OK);

    CHECK(str_eq(rattler_match_spec_name(spec), "numpy"));
    CHECK(str_eq(rattler_match_spec_version(spec), ">=1.20,<2"));
    CHECK(str_eq(rattler_match_spec_build(spec), "py312*"));
    CHECK(str_eq(rattler_match_spec_channel(spec), "conda-forge"));

    RattlerStr subdir = rattler_match_spec_subdir(spec);
    CHECK(subdir.data == NULL);
    rattler_str_free(subdir);

    RattlerVersion *version = parse_version("1.26.4");
    CHECK(rattler_match_spec_matches_version(spec, version));

    bool matches = false;
    const char *name = "numpy";
    const char *build = "py312h1234_0";
    CHECK(rattler_match_spec_matches_package(spec, name, strlen(name), version,
                                             build, strlen(build), 0,
                                             &matches) == RATTLER_STATUS_OK);
    CHECK(matches);

    rattler_version_free(version);
    rattler_match_spec_free(spec);
}

int main(void) {
    printf("rattler %s\n", rattler_library_version());
    test_version();
    test_version_error();
    test_match_spec();
    if (failures > 0) {
        fprintf(stderr, "%d check(s) failed\n", failures);
        return 1;
    }
    printf("all checks passed\n");
    return 0;
}
