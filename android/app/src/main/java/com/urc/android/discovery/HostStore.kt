package com.urc.android.discovery

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import org.json.JSONArray

private val Context.dataStore: DataStore<Preferences> by preferencesDataStore(name = "urc_hosts")

/**
 * Persists the saved-host list as a JSON array string in a single DataStore
 * preference key. Dedup is by [Host.key] (ip:port). Adding a host that already
 * exists updates its label fields rather than duplicating.
 */
class HostStore(private val context: Context) {

    private val key = stringPreferencesKey("hosts_json")

    val hosts: Flow<List<Host>> = context.dataStore.data.map { prefs ->
        decode(prefs[key])
    }

    suspend fun list(): List<Host> = hosts.first()

    suspend fun add(host: Host) {
        context.dataStore.edit { prefs ->
            val current = decode(prefs[key]).toMutableList()
            val idx = current.indexOfFirst { it.key == host.key }
            if (idx >= 0) current[idx] = host else current.add(host)
            prefs[key] = encode(current)
        }
    }

    suspend fun remove(host: Host) {
        context.dataStore.edit { prefs ->
            val current = decode(prefs[key]).filterNot { it.key == host.key }
            prefs[key] = encode(current)
        }
    }

    private fun encode(list: List<Host>): String {
        val arr = JSONArray()
        list.forEach { arr.put(it.toJson()) }
        return arr.toString()
    }

    private fun decode(raw: String?): List<Host> {
        if (raw.isNullOrBlank()) return emptyList()
        return try {
            val arr = JSONArray(raw)
            (0 until arr.length()).mapNotNull { i ->
                runCatching { Host.fromJson(arr.getJSONObject(i)) }.getOrNull()
            }
        } catch (e: Exception) {
            emptyList()
        }
    }
}
