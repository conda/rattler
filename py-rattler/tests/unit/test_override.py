from rattler import VirtualPackage, VirtualPackageOverrides, Override, Version

def test_stuff():
    overrides = VirtualPackageOverrides.none()
    assert overrides.osx == Override.none()
    assert overrides.libc == Override.none()
    assert overrides.cuda == Override.none()
    overrides = VirtualPackageOverrides.default()
    assert overrides.osx == Override.default()
    assert overrides.libc == Override.default()
    assert overrides.cuda == Override.default()
    
    overrides.osx = Override.string("123.45")
    overrides.libc = Override.string("123.457")
    overrides.cuda = Override.string("123.4578")

    r = VirtualPackage.current_with_overrides(overrides)
    def find(name, ver):
        for i in r:
            if i.name == name:
                assert i.version == Version(ver)
        assert False
    
    find("__cuda", "123.4578")
    find("__libc", "123.457")
    find("__osx", "123.45")