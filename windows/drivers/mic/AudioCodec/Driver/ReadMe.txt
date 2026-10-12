========================================================================
    AudioCodec Project Overview
========================================================================

This project builds the NTPods ACX virtual microphone from the Microsoft
AudioCodec sample. See ../../README.md for the driver contract and build steps.

AudioCodec.vcxproj
    Build configuration, source files and dependencies.

AudioCodec.vcxproj.filters
    Visual Studio source-file groups.

Driver.cpp
    DriverEntry and WDFDRIVER callbacks.

Device.cpp
    WDFDEVICE callbacks and capture-device setup.

../../Common/
    Capture circuit, PCM bridge, stream engine and shared driver headers.

DriverSettings.h
    Device and circuit GUIDs.

/////////////////////////////////////////////////////////////////////////////

Learn more about Kernel Mode Driver Framework here:

http://msdn.microsoft.com/en-us/library/ff544296(v=VS.85).aspx

/////////////////////////////////////////////////////////////////////////////
