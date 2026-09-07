import databaseSchema from "/D1.sql?raw";

export async function initializeTestDatabase(database: D1Database): Promise<void> {
    const statements = databaseSchema
        .split(/;\s*(?:\n|$)/)
        .map((statement) => statement.trim())
        .filter((statement) => statement.length > 0)
        .map((statement) => database.prepare(statement));
    await database.batch(statements);
}

export async function clearTestDatabase(database: D1Database): Promise<void> {
    await database.prepare("DELETE FROM oauth_states").run();
    // The models FK cascades; leftover model rows would therefore fail the next test loudly.
    await database.prepare("DELETE FROM providers").run();
}
