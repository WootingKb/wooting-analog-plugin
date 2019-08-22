# Wooting Analog SDK Plugin

This is Wooting's plugin for the [Analog SDK](https://github.com/simon-wh/Analog-SDK) providing easily extensible support for the Wooting One and Wooting Two.

NOTE: At the moment you need an beta firmware for the Wooting One and Wooting Two which hasn't been pushed to Wootility for this to work.

## Setting up
For the Plugin to work you must add the directory it is in to the `WOOTING_ANALOG_SDK_PLUGINS_PATH` environment variable. The `.msi` and `.deb` installers will do this for you, but if you compile it manually or download just the plugin you'll need to do this yourself.

For bash/Linux:
```sh
export WOOTING_ANALOG_SDK_PLUGINS_PATH=${WOOTING_ANALOG_SDK_PLUGINS_PATH};/path/to/plugin
```

For Powershell:
```ps
$Env:WOOTING_ANALOG_SDK_PLUGINS_PATH += ";C:\path\to\plugin"
```

> Setting the variable like this is only temporary and will only apply to your current terminal session