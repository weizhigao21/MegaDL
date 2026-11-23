# MegaDL Android 构建脚本（不依赖 Gradle：aapt2 + javac + d8 + apksigner）
# 需要 JDK 17、Android SDK（platform 34 + build-tools 34.0.0）
$ErrorActionPreference = "Stop"

$sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } elseif (Test-Path "C:\Android") { "C:\Android" } else { "" }
if (-not $sdk) { throw "未找到 Android SDK，请设置 ANDROID_HOME" }
$javaHome = if ($env:JAVA_HOME) { $env:JAVA_HOME } else { (Get-ChildItem "C:\Program Files\Microsoft" -Directory -Filter "jdk-17*" | Select-Object -First 1).FullName }

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $root
$build = "build"
$aapt2 = Join-Path $sdk "build-tools\34.0.0\aapt2.exe"
$androidJar = Join-Path $sdk "platforms\android-34\android.jar"
$d8 = Join-Path $sdk "build-tools\34.0.0\d8.bat"
$zipalign = Join-Path $sdk "build-tools\34.0.0\zipalign.exe"
$apksigner = Join-Path $sdk "build-tools\34.0.0\apksigner.bat"

New-Item -ItemType Directory -Force $build, "$build\gen", "$build\classes", "$build\dex" | Out-Null
Remove-Item "$build\classes\*", "$build\dex\*", "$build\gen\*" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "$build\res.zip" -Force -ErrorAction SilentlyContinue

Write-Host "[1/5] 编译资源"
# aapt2 在含中文的绝对路径上会报 “failed to open directory”，这里统一用相对路径
& $aapt2 compile --dir res -o "$build\res.zip"
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "aapt2 compile 失败（$LASTEXITCODE）" }
& $aapt2 link -o "$build\base.apk" -I $androidJar --manifest AndroidManifest.xml `
    --java "$build\gen" --min-sdk-version 24 --target-sdk-version 34 `
    --version-code 3 --version-name 0.2.1 "$build\res.zip"
if ($LASTEXITCODE -ne 0) { Pop-Location; throw "aapt2 link 失败（$LASTEXITCODE）" }

Write-Host "[2/5] 编译 Java"
$srcs = Get-ChildItem (Join-Path $root "src") -Recurse -Filter *.java | Select-Object -ExpandProperty FullName
& "$javaHome\bin\javac.exe" -encoding UTF-8 --release 8 -classpath $androidJar -d "$build\classes" $srcs "$build\gen\com\weizhigao21\megadl\R.java"
if ($LASTEXITCODE -ne 0) { throw "javac 失败（$LASTEXITCODE）" }

Write-Host "[3/5] dex"
$classes = Get-ChildItem "$build\classes" -Recurse -Filter *.class | Select-Object -ExpandProperty FullName
& $d8 --lib $androidJar --min-api 24 --output "$build\dex" $classes
if ($LASTEXITCODE -ne 0) { throw "d8 失败（$LASTEXITCODE）" }

Write-Host "[4/5] 打包"
Copy-Item "$build\base.apk" "$build\unsigned.apk" -Force
& "$javaHome\bin\jar.exe" uf "$build\unsigned.apk" -C "$build\dex" classes.dex
if ($LASTEXITCODE -ne 0) { throw "打包 classes.dex 失败（$LASTEXITCODE）" }
& $zipalign -f 4 "$build\unsigned.apk" "$build\aligned.apk"
if ($LASTEXITCODE -ne 0) { throw "zipalign 失败（$LASTEXITCODE）" }

Write-Host "[5/5] 签名"
$ks = Join-Path $build "debug.keystore"
if (-not (Test-Path $ks)) {
    & "$javaHome\bin\keytool.exe" -genkeypair -keystore $ks -storepass android -keypass android `
        -alias androiddebugkey -dname "CN=Android Debug,O=Android,C=US" `
        -keyalg RSA -keysize 2048 -validity 20000 | Out-Null
}
& $apksigner sign --ks $ks --ks-pass pass:android --key-pass pass:android `
    --out "$build\MegaDL.apk" "$build\aligned.apk"
& $apksigner verify "$build\MegaDL.apk"

$size = "{0:N1} KB" -f ((Get-Item "$build\MegaDL.apk").Length / 1KB)
Pop-Location
Write-Host "生成：$root\$build\MegaDL.apk（$size）"
