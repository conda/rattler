from rattler import VirtualPackage, VirtualPackageOverrides, Override, Version, PackageName

def test_stuff() -> None:
    overrides = VirtualPackageOverrides.none()
    print(overrides.osx, Override.none())
    assert overrides.osx == Override.none()
    assert overrides.libc == Override.none()
    assert overrides.cuda == Override.none()
    overrides = VirtualPackageOverrides.default()
    assert overrides.osx == Override.default_env_var()
    assert overrides.libc == Override.default_env_var()
    assert overrides.cuda == Override.default_env_var()
    
    overrides.osx = Override.string("123.45")
    overrides.libc = Override.string("123.457")
    overrides.cuda = Override.string("123.4578")

    r = [i.into_generic() for i in VirtualPackage.current_with_overrides(overrides)]
    def find(name: str, ver: str, must_find: bool=True) -> None:
        for i in r:
            if i.name == PackageName(name):
                assert i.version == Version(ver)
                return
        assert not must_find
    
    find("__cuda", "123.4578")
    find("__libc", "123.4578", False)
    find("__osx", "123.45", False)