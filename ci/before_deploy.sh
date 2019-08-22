# This script takes care of building your crate and packaging it for release

set -ex

main() {
    local src=$(pwd) 
          stage=
          lib_ext=
          lib_prefix=
          shared_lib_ext=
          cargo=cargo

    case $TRAVIS_OS_NAME in
        linux)
            stage=$(mktemp -d)
            lib_ext="a"
            lib_prefix="lib"
            shared_lib_ext="so"
            cargo=cross
            ;;
        osx)
            stage=$(mktemp -d -t tmp)
            lib_ext="a"
            lib_prefix="lib"
            shared_lib_ext="dylib"
            cargo=cross
            ;;
        windows)
            stage=$(mktemp -d)
            lib_ext="lib"
            lib_prefix=""
            shared_lib_ext="dll"
            ;;
    esac

    test -f Cargo.lock || cargo generate-lockfile

    cargo build --target $TARGET --release



    # Copy Plugin items
    cp target/$TARGET/release/${lib_prefix}wooting_analog_plugin.$lib_ext $stage
    cp README.md $stage

    cd $stage
    tar czf $src/$CRATE_NAME-$TRAVIS_TAG-$TARGET.tar.gz *
    cd $src

    rm -rf $stage
}

main
