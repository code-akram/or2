package io.github.code_akram.or2.data

import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase

/**
 * v1 -> v2: a host's single `hostname`/`port` become its address at position 0, and hosts gain
 * `showInInbox` (default true, so every existing host appears in the inbox).
 *
 * Never destructive: key records are bound to Keystore entries that cannot be recreated, and
 * trust must survive an upgrade (the destination is unchanged). `hosts` is altered in place and
 * never dropped or recreated: with foreign keys on, `DROP TABLE hosts` deletes every row first
 * and would cascade away `trusted_host_keys`. `DROP COLUMN` needs SQLite 3.35. Android 14 (the
 * minSdk) is documented to ship 3.42, but the JVM tests run their own SQLite, so
 * `MigrationDeviceTest.theDevicesSqliteSupportsDropColumn` is the check on the real device.
 */
val MIGRATION_1_2_STATEMENTS = listOf(
    "CREATE TABLE IF NOT EXISTS `host_addresses` (`hostId` INTEGER NOT NULL, `position` INTEGER NOT NULL, " +
        "`hostname` TEXT NOT NULL, `port` INTEGER NOT NULL, PRIMARY KEY(`hostId`, `position`), " +
        "FOREIGN KEY(`hostId`) REFERENCES `hosts`(`id`) ON UPDATE NO ACTION ON DELETE CASCADE )",
    "INSERT INTO `host_addresses` (`hostId`, `position`, `hostname`, `port`) SELECT `id`, 0, `hostname`, `port` FROM `hosts`",
    "ALTER TABLE `hosts` ADD COLUMN `showInInbox` INTEGER NOT NULL DEFAULT 1",
    "ALTER TABLE `hosts` DROP COLUMN `hostname`",
    "ALTER TABLE `hosts` DROP COLUMN `port`",
)

val MIGRATION_1_2 = object : Migration(1, 2) {
    override fun migrate(db: SupportSQLiteDatabase) {
        MIGRATION_1_2_STATEMENTS.forEach(db::execSQL)
    }
}

/**
 * v2 -> v3: each host gains `transport` (`AUTO`, `SSH` or `MOSH`; existing hosts get `AUTO`).
 * One additive `ALTER TABLE ... ADD COLUMN`: nothing is dropped, recreated or rewritten, so keys,
 * trust and addresses are untouched.
 */
val MIGRATION_2_3_STATEMENTS = listOf(
    "ALTER TABLE `hosts` ADD COLUMN `transport` TEXT NOT NULL DEFAULT 'AUTO'",
)

val MIGRATION_2_3 = object : Migration(2, 3) {
    override fun migrate(db: SupportSQLiteDatabase) {
        MIGRATION_2_3_STATEMENTS.forEach(db::execSQL)
    }
}
