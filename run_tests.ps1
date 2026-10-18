$msvc = "E:\project\build\VC\Tools\MSVC\14.51.36231"
$sdk = "C:\Program Files (x86)\Windows Kits\10"
$sdkVer = "10.0.26100.0"

$env:Path = "$msvc\bin\Hostx64\x64;" + $env:Path
$env:LIB = "$msvc\lib\x64;$sdk\Lib\$sdkVer\ucrt\x64;$sdk\Lib\$sdkVer\um\x64;" + $env:LIB
$env:INCLUDE = "$msvc\include;$sdk\Include\$sdkVer\ucrt;$sdk\Include\$sdkVer\shared;$sdk\Include\$sdkVer\um;" + $env:INCLUDE

cargo test --features jni --test standard_api_test
