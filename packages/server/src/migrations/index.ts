import type { DatabaseSync } from 'node:sqlite';
import { m001Groups } from './001-groups.js';
import { m002Devices } from './002-devices.js';
import { m003Remote } from './003-remote.js';
import { m004Explore } from './004-explore.js';
import { m005Albums } from './005-albums.js';

/**
 * Schema migrations, applied in order on top of the base SCHEMA (db.ts).
 * PRAGMA user_version = number of applied migrations.
 * Each migration runs in its own transaction; the DB file is backed up first.
 * Never edit a released migration — append a new one.
 */
export interface Migration {
  name: string;
  up(db: DatabaseSync): void;
}

export const MIGRATIONS: Migration[] = [m001Groups, m002Devices, m003Remote, m004Explore, m005Albums];
