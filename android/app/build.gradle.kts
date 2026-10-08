import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
    id("com.google.devtools.ksp")
}

val rustRoot = rootProject.file("../core")
val generatedBindings = layout.buildDirectory.dir("generated/uniffi/kotlin")
val generatedLibraries = layout.buildDirectory.dir("generated/uniffi/jniLibs")
val rustInputs = fileTree(rustRoot) {
    include("Cargo.toml", "Cargo.lock", "or2-core/**", "or2-ffi/**")
}
val pairInputs = fileTree(rustRoot) {
    include("Cargo.toml", "Cargo.lock", "or2-core/**", "or2-pair/**")
}

val buildRustHost by tasks.registering(Exec::class) {
    workingDir(rustRoot)
    commandLine("cargo", "build", "--locked", "-p", "or2-ffi", "--lib")
    inputs.files(rustInputs)
    outputs.file(rustRoot.resolve("target/debug/libor2_ffi.so"))
}

// The host CLI with its test-only auto-confirming host: the JVM end-to-end test of Easy pair talks to
// it over loopback. It is never part of the app or of `cargo install or2-pair`.
val buildPairTesthost by tasks.registering(Exec::class) {
    workingDir(rustRoot)
    commandLine("cargo", "build", "--locked", "-p", "or2-pair", "--features", "test-support", "--bin", "or2-pair-testhost")
    inputs.files(pairInputs)
    outputs.file(rustRoot.resolve("target/debug/or2-pair-testhost"))
}

val generateRustBindings by tasks.registering(Exec::class) {
    dependsOn(buildRustHost)
    workingDir(rustRoot)
    commandLine(
        "cargo", "run", "--locked", "-p", "or2-ffi", "--features", "bindgen",
        "--bin", "uniffi-bindgen", "--", "generate", "target/debug/libor2_ffi.so",
        "--language", "kotlin", "--out-dir", generatedBindings.get().asFile,
        "--no-format",
    )
    inputs.files(rustInputs)
    outputs.dir(generatedBindings)
}

// The library keeps no build-machine paths: the repository and the cargo home are remapped (as `xtask dist`
// does for or2-pair), and the build fails if either path is still in the library.
val repoRoot: File = rootProject.file("..").canonicalFile
val cargoHome: File = (providers.environmentVariable("CARGO_HOME").orNull?.takeIf { it.isNotBlank() }?.let(::File)
    ?: File(System.getProperty("user.home"), ".cargo")).canonicalFile

val buildRustAndroid by tasks.registering(Exec::class) {
    workingDir(rustRoot)
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "--platform", "34",
        "-o", generatedLibraries.get().asFile,
        "build", "--release", "--locked", "-p", "or2-ffi", "--lib",
    )
    environment(
        "CARGO_ENCODED_RUSTFLAGS",
        listOf("--remap-path-prefix=$repoRoot=/or2", "--remap-path-prefix=$cargoHome=/cargo").joinToString("\u001f"),
    )
    inputs.files(rustInputs)
    outputs.dir(generatedLibraries)
    doLast {
        val library = generatedLibraries.get().asFile.resolve("arm64-v8a/libor2_ffi.so")
        // Latin-1 maps each byte to one char, so a byte search is a string search.
        val contents = String(library.readBytes(), Charsets.ISO_8859_1)
        for (path in listOf(repoRoot, cargoHome)) {
            val needle = String(path.path.toByteArray(), Charsets.ISO_8859_1)
            if (contents.contains(needle)) throw GradleException("$library still contains the build path $path")
        }
    }
}

// Release signing (docs/build.md, "Release signing"). The properties file named by OR2_SIGNING_PROPERTIES,
// else $XDG_CONFIG_HOME/or2/signing.properties (~/.config/or2/signing.properties), gives storeFile (relative
// to that file's directory, or absolute), storePassword, keyAlias and keyPassword. Without the file the release
// build stays unsigned, as F-Droid builds it from source. No key or password is in the repository, and none of
// the values is ever printed.
class ReleaseSigning(val storeFile: File, val storePassword: String, val keyAlias: String, val keyPassword: String)

val releaseSigning: ReleaseSigning? = run {
    val named = providers.environmentVariable("OR2_SIGNING_PROPERTIES").orNull?.takeIf { it.isNotBlank() }
    val configHome = providers.environmentVariable("XDG_CONFIG_HOME").orNull?.takeIf { it.isNotBlank() }
        ?: "${System.getProperty("user.home")}/.config"
    val file = File(named ?: "$configHome/or2/signing.properties")
    if (!file.isFile) return@run null
    val properties = Properties().apply { file.inputStream().use { stream -> load(stream) } }
    fun value(key: String): String = properties.getProperty(key)?.takeIf { it.isNotEmpty() }
        ?: throw GradleException("$file sets no $key (it needs storeFile, storePassword, keyAlias and keyPassword)")
    val store = File(value("storeFile")).let { if (it.isAbsolute) it else file.parentFile.resolve(it) }
    if (!store.isFile) throw GradleException("the keystore that $file names does not exist: $store")
    ReleaseSigning(store, value("storePassword"), value("keyAlias"), value("keyPassword"))
}

android {
    namespace = "io.github.code_akram.or2"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "io.github.code_akram.or2"
        minSdk = 34
        targetSdk = 36
        versionCode = 13
        versionName = "0.1.7"
        testInstrumentationRunner = "io.github.code_akram.or2.Or2TestRunner"
        manifestPlaceholders["appLabel"] = "or2"
        ndk { abiFilters += "arm64-v8a" }
    }

    // Device tests never touch the owner's app. The debug build is a daily app
    // (io.github.code_akram.or2, its hosts and Keystore keys), and a connected test run installs over
    // and then uninstalls the app under test. So the instrumented tests target their own build type:
    // debug in every respect (the same signing, debug-only sources and dependencies) but installed as
    // io.github.code_akram.or2.devicetest, with its own data, Keystore and permissions. The debug
    // variant has no androidTest at all, so no task can install a test APK against the daily app.
    releaseSigning?.let { signing ->
        val config = signingConfigs.create("release") {
            storeFile = signing.storeFile
            storePassword = signing.storePassword
            keyAlias = signing.keyAlias
            keyPassword = signing.keyPassword
        }
        buildTypes.getByName("release").signingConfig = config
    }
    buildTypes {
        create("deviceTest") {
            initWith(getByName("debug"))
            applicationIdSuffix = ".devicetest"
            manifestPlaceholders["appLabel"] = "or2 devicetest"
            matchingFallbacks += "debug"
        }
    }
    testBuildType = "deviceTest"

    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets["main"].apply {
        java.srcDir(generatedBindings)
        jniLibs.srcDir(generatedLibraries)
    }
    // deviceTest builds the debug-only code (TerminalProbeActivity, UiGalleryActivity) from the debug
    // source set itself, so the tests exercise exactly what the debug app ships.
    sourceSets["deviceTest"].apply {
        java.srcDir("src/debug/java")
        manifest.srcFile("src/debug/AndroidManifest.xml")
    }
    // MigrationTestHelper reads the exported schemas as instrumented-test assets.
    sourceSets["androidTest"].assets.srcDir("$projectDir/schemas")
}

ksp {
    // Room schema JSONs are checked in so migrations can be tested against every shipped version.
    arg("room.schemaLocation", "$projectDir/schemas")
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

configurations.configureEach {
    if (name.endsWith("RuntimeClasspath")) {
        // Kotlin 2.2 common is legacy metadata, not a separate JVM runtime.
        // Its redundant runtime record breaks Gradle's lock-state validation.
        exclude(group = "org.jetbrains.kotlin", module = "kotlin-stdlib-common")
    }
}

tasks.named("preBuild") {
    dependsOn(generateRustBindings, buildRustAndroid)
}

tasks.withType<Test>().configureEach {
    dependsOn(buildRustHost, buildPairTesthost)
    systemProperty("jna.library.path", rustRoot.resolve("target/debug").absolutePath)
    systemProperty("or2.pair.testhost", rustRoot.resolve("target/debug/or2-pair-testhost").absolutePath)
}

dependencies {
    implementation("androidx.activity:activity-compose:1.11.0")
    implementation("androidx.fragment:fragment-ktx:1.8.9")
    implementation("androidx.biometric:biometric:1.1.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-ktx:2.9.4")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.9.4")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    implementation("androidx.room:room-runtime:2.8.3")
    ksp("androidx.room:room-compiler:2.8.3")
    implementation(platform("androidx.compose:compose-bom:2025.10.00"))
    // room-testing 2.8.3 (androidTest) brings serialization-json 1.8.1, whose generated serializers
    // call GeneratedSerializer methods that core 1.7.3 lacks; consistent resolution holds the
    // androidTest runtime at the tested (deviceTest) runtime's version, so align the group for the
    // debug-like builds only: the shipped release runtime keeps the version its own dependencies ask
    // for (1.7.3). debug keeps it so deviceTest stays identical to debug.
    debugImplementation(platform("org.jetbrains.kotlinx:kotlinx-serialization-bom:1.8.1"))
    "deviceTestImplementation"(platform("org.jetbrains.kotlinx:kotlinx-serialization-bom:1.8.1"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    // Easy pair: the camera preview and frames (AndroidX, Apache-2.0) and the QR decoder (ZXing core,
    // Apache-2.0, pure Java). No Google Play Services: CameraX's camera2 backend and ZXing instead of ML Kit.
    implementation("androidx.camera:camera-core:1.5.3")
    implementation("androidx.camera:camera-camera2:1.5.3")
    implementation("androidx.camera:camera-lifecycle:1.5.3")
    implementation("androidx.camera:camera-view:1.5.3")
    implementation("com.google.zxing:core:3.5.4")

    testImplementation("junit:junit:4.13.2")
    // Runs the Room migration SQL on real SQLite (Apache-2.0; test-only).
    testImplementation("org.xerial:sqlite-jdbc:3.53.4.0")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2")
    testRuntimeOnly("net.java.dev.jna:jna:5.17.0")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.room:room-testing:2.8.3")
    androidTestImplementation(platform("androidx.compose:compose-bom:2025.10.00"))
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    // Declares the empty ComponentActivity that createComposeRule() hosts tests in (debug-like only).
    debugImplementation("androidx.compose.ui:ui-test-manifest")
    "deviceTestImplementation"("androidx.compose.ui:ui-test-manifest")
}
