package io.github.code_akram.or2.app

import android.app.Application
import androidx.room.Room
import io.github.code_akram.or2.data.AppDatabase
import io.github.code_akram.or2.ffi.connect
import io.github.code_akram.or2.keys.BiometricVault
import io.github.code_akram.or2.session.SessionConnector
import io.github.code_akram.or2.session.SessionHolder

class Or2Application : Application() {
    val database by lazy { Room.databaseBuilder(this, AppDatabase::class.java, "or2.db").build() }
    val vault by lazy { BiometricVault(this) }
    val sessions by lazy {
        SessionHolder(SessionConnector(::connect), database.dao())
    }
}
