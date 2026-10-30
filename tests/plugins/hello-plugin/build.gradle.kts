plugins { java }

repositories {
    maven { url = uri("https://repo.papermc.io/repository/maven-public/") }
}

dependencies {
    compileOnly("io.papermc.paper:paper-api:26.3.build.205-stable")
}

java { toolchain { languageVersion = JavaLanguageVersion.of(25) } }
tasks.jar { archiveFileName.set("ferrummc-hello.jar") }
