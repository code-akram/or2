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

val buildRustHost by tasks.registering(Exec::class) {
    workingDir(rustRoot)
    commandLine("cargo", "build", "--locked", "-p", "or2-ffi", "--lib")
    inputs.files(rustInputs)
    outputs.file(rustRoot.resolve("target/debug/libor2_ffi.so"))
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

val buildRustAndroid by tasks.registering(Exec::class) {
    workingDir(rustRoot)
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "--platform", "34",
        "-o", generatedLibraries.get().asFile,
        "build", "--release", "--locked", "-p", "or2-ffi", "--lib",
    )
    inputs.files(rustInputs)
    outputs.dir(generatedLibraries)
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
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "io.github.code_akram.or2.Or2TestRunner"
        ndk { abiFilters += "arm64-v8a" }
    }

    buildFeatures { compose = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets["main"].apply {
        java.srcDir(generatedBindings)
        jniLibs.srcDir(generatedLibraries)
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
    dependsOn(buildRustHost)
    systemProperty("jna.library.path", rustRoot.resolve("target/debug").absolutePath)
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
    // androidTest runtime at the debug runtime's version, so align the group for debug builds only:
    // the shipped release runtime keeps the version its own dependencies ask for (1.7.3).
    debugImplementation(platform("org.jetbrains.kotlinx:kotlinx-serialization-bom:1.8.1"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("net.java.dev.jna:jna:5.17.0@aar")

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
}
