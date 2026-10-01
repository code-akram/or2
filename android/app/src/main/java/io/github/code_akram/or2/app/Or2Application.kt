package io.github.code_akram.or2.app

import android.app.Application
import androidx.room.Room
import io.github.code_akram.or2.connection.HostConnections
import io.github.code_akram.or2.connection.HostConnector
import io.github.code_akram.or2.data.AppDatabase
import io.github.code_akram.or2.data.MIGRATION_1_2
import io.github.code_akram.or2.keys.BiometricVault

class Or2Application : Application() {
    val database by lazy { Room.databaseBuilder(this, AppDatabase::class.java, "or2.db")
        // Never destructive: key records are bound to Keystore entries that cannot be recreated.
        .addMigrations(MIGRATION_1_2).build() }
    val vault by lazy { BiometricVault(this) }
    val connections by lazy { HostConnections(HostConnector.Native, database.dao()) }
}
