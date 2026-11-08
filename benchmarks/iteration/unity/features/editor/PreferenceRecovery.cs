using System;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using AraIteration;
using Unity.Burst;
using UnityEditor;
using UnityEngine;

namespace AraFeatureControl
{
    /// <summary>Feature-only durable preference checkpoint and explicit batch recovery.</summary>
    public static class PreferenceRecovery
    {
        static readonly string[] Keys = { "BurstCompilation", "BurstCompileSynchronously", "BurstShowTimings", "BurstDebug", "BurstForceSafetyChecks" };
        [Serializable] sealed class Marker { public string repository_root, fixture; }
        [Serializable] sealed class Checkpoint
        {
            public int version = 1;
            public string invocation_id, project, result_path, initial_json, initial_state_sha256;
        }
        [Serializable] sealed class Receipt
        {
            public int version = 1;
            public string kind, invocation_id, checkpoint_sha256, initial_state_sha256, project, result_path, recovery_attempt_id;
            public bool restored, initial_options_restored, known_preferences_restored;
        }

        static string Hash(byte[] bytes)
        {
            using var sha = SHA256.Create();
            return BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant();
        }

        static string Project()
        {
            if (!Application.isBatchMode) throw new InvalidOperationException("preference control requires batch mode");
            string project = Directory.GetParent(Application.dataPath).FullName;
            var marker = JsonUtility.FromJson<Marker>(File.ReadAllText(Path.Combine(project, ".ara-query-feature.json")));
            if (marker == null || string.IsNullOrEmpty(marker.repository_root) ||
                (marker.fixture != "query_filter-v1" && marker.fixture != "extended-v1") ||
                !project.StartsWith(Path.GetFullPath(Path.Combine(marker.repository_root, "target")) + Path.DirectorySeparatorChar, StringComparison.Ordinal))
                throw new InvalidOperationException("unowned preference recovery project");
            return project;
        }

        static string Bound(string project, string raw)
        {
            string path = Path.GetFullPath(raw);
            if (!path.StartsWith(Path.Combine(project, "results") + Path.DirectorySeparatorChar, StringComparison.Ordinal))
                throw new InvalidOperationException("checkpoint/result must stay in owned scratch results");
            for (var parent = new DirectoryInfo(Path.GetDirectoryName(path)); parent != null && parent.FullName.StartsWith(project, StringComparison.Ordinal); parent = parent.Parent)
                if (parent.Exists && (parent.Attributes & FileAttributes.ReparsePoint) != 0) throw new InvalidOperationException("checkpoint path contains a symlink");
            if (File.Exists(path) && (File.GetAttributes(path) & FileAttributes.ReparsePoint) != 0) throw new InvalidOperationException("checkpoint file is a symlink");
            return path;
        }

        static void Atomic(string path, string text)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(path));
            string temporary = path + ".tmp-" + Guid.NewGuid().ToString("N");
            try
            {
                using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None))
                {
                    byte[] bytes = Encoding.UTF8.GetBytes(text); stream.Write(bytes, 0, bytes.Length); stream.Flush(true);
                }
                File.Move(temporary, path); // Same-directory atomic rename; never overwrite an earlier invocation.
            }
            finally { if (File.Exists(temporary)) File.Delete(temporary); }
        }

        static MacBatch.EnvironmentRow Initial(Checkpoint checkpoint)
        {
            if (checkpoint.version != 1 || string.IsNullOrEmpty(checkpoint.invocation_id) ||
                Hash(Encoding.UTF8.GetBytes(checkpoint.initial_json)) != checkpoint.initial_state_sha256)
                throw new InvalidOperationException("invalid checkpoint initial-state binding");
            var initial = JsonUtility.FromJson<MacBatch.EnvironmentRow>(checkpoint.initial_json);
            if (initial.measurement_completed) throw new InvalidOperationException("checkpoint must precede measurement");
            var preferences = initial.initial_editor_preferences;
            if (preferences == null || preferences.Length != Keys.Length ||
                !preferences.Select(row => row.key).OrderBy(key => key, StringComparer.Ordinal).SequenceEqual(Keys.OrderBy(key => key, StringComparer.Ordinal)))
                throw new InvalidOperationException("checkpoint must contain exactly the five known keys");
            return initial;
        }

        static bool PreferencesMatch(MacBatch.PreferenceRow[] rows) => rows.All(row =>
            EditorPrefs.HasKey(row.key) == row.existed && (!row.existed || EditorPrefs.GetBool(row.key, false) == row.value));
        static bool OptionsMatch(MacBatch.EnvironmentRow initial)
        {
            var options = BurstCompiler.Options;
            return options.EnableBurstCompilation == initial.initial_burst_compilation && options.EnableBurstSafetyChecks == initial.initial_burst_safety_checks &&
                options.ForceEnableBurstSafetyChecks == initial.initial_burst_force_safety_checks && options.EnableBurstDebug == initial.initial_burst_debug &&
                options.EnableBurstCompileSynchronously == initial.initial_burst_compile_synchronously;
        }
        static void RestoreOptions(MacBatch.EnvironmentRow initial)
        {
            var options = BurstCompiler.Options;
            try { options.EnableBurstCompilation = initial.initial_burst_compilation; }
            finally { try { options.EnableBurstSafetyChecks = initial.initial_burst_safety_checks; }
                finally { try { options.ForceEnableBurstSafetyChecks = initial.initial_burst_force_safety_checks; }
                    finally { try { options.EnableBurstDebug = initial.initial_burst_debug; }
                        finally { options.EnableBurstCompileSynchronously = initial.initial_burst_compile_synchronously; } } } }
        }

        /// <summary>Flushes the captured initial state before the wrapper may invoke any option setter.</summary>
        public static void Save(string environmentPath, MacBatch.EnvironmentRow initial)
        {
            string project = Project(); string result = Bound(project, environmentPath.Substring(0, environmentPath.Length - ".environment.json".Length));
            string initialJson = JsonUtility.ToJson(initial);
            var checkpoint = new Checkpoint { invocation_id = Guid.NewGuid().ToString("N"), project = project, result_path = result,
                initial_json = initialJson, initial_state_sha256 = Hash(Encoding.UTF8.GetBytes(initialJson)) };
            Initial(checkpoint);
            string path = Bound(project, environmentPath + ".initial.json"); Atomic(path, JsonUtility.ToJson(checkpoint, true));
            Debug.Log("ARA_FEATURE_CHECKPOINT_SAVED " + path);
        }

        static void ReceiptFor(string checkpointPath, string kind, bool noErrors, string recoveryAttempt = null)
        {
            string project = Project(); checkpointPath = Bound(project, checkpointPath);
            byte[] raw = File.ReadAllBytes(checkpointPath); var checkpoint = JsonUtility.FromJson<Checkpoint>(Encoding.UTF8.GetString(raw));
            if (checkpoint.project != project || checkpointPath != Bound(project, checkpoint.result_path) + ".environment.json.initial.json")
                throw new InvalidOperationException("checkpoint belongs to another invocation/project");
            var initial = Initial(checkpoint); bool preferences = PreferencesMatch(initial.initial_editor_preferences), options = OptionsMatch(initial);
            var receipt = new Receipt { kind = kind, invocation_id = checkpoint.invocation_id, project = project, result_path = checkpoint.result_path, recovery_attempt_id = recoveryAttempt,
                checkpoint_sha256 = Hash(raw), initial_state_sha256 = checkpoint.initial_state_sha256,
                restored = noErrors && preferences && options, known_preferences_restored = preferences, initial_options_restored = options };
            Atomic(checkpointPath + (kind == "normal_finally" ? ".restoration.json" : ".recovery-" + recoveryAttempt + ".json"), JsonUtility.ToJson(receipt, true));
            if (!receipt.restored) throw new InvalidOperationException("recorded preference/option restoration not proven");
        }

        /// <summary>Writes a separate completion receipt after normal finally restoration.</summary>
        public static void Complete(string environmentPath, MacBatch.EnvironmentRow environment)
            => ReceiptFor(environmentPath + ".initial.json", "normal_finally", environment.finally_restored);

        /// <summary>Restores only the recorded options and five whitelisted keys, then exits without a benchmark.</summary>
        public static void Recover()
        {
            if (!Application.isBatchMode) { Debug.LogError("preference recovery requires batch mode"); return; }
            int exit = 1;
            try
            {
                string project = Project(); string[] args = Environment.GetCommandLineArgs(); int at = Array.IndexOf(args, "-ara-feature-recover") + 1;
                if (at == 0 || args.Length < at + 3 || args[at + 2].Length != 32 || !args[at + 2].All(Uri.IsHexDigit)) throw new ArgumentException("missing checkpoint/hash/recovery attempt");
                string path = Bound(project, args[at]); byte[] raw = File.ReadAllBytes(path);
                if (Hash(raw) != args[at + 1]) throw new InvalidOperationException("checkpoint changed before recovery");
                var checkpoint = JsonUtility.FromJson<Checkpoint>(Encoding.UTF8.GetString(raw));
                if (checkpoint.project != project || path != Bound(project, checkpoint.result_path) + ".environment.json.initial.json") throw new InvalidOperationException("checkpoint invocation differs");
                var initial = Initial(checkpoint); bool noErrors = false;
                try { RestoreOptions(initial); noErrors = true; }
                finally
                {
                    foreach (var preference in initial.initial_editor_preferences)
                        if (preference.existed) EditorPrefs.SetBool(preference.key, preference.value); else EditorPrefs.DeleteKey(preference.key);
                    ReceiptFor(path, "explicit_recovery", noErrors, args[at + 2]);
                }
                exit = 0;
            }
            catch (Exception error) { Debug.LogException(error); }
            EditorApplication.Exit(exit);
        }
    }
}
