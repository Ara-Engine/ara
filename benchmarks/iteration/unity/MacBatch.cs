// Editor-only provenance wrapper. S8's benchmark math and timed loop are unchanged.
using System;
using System.IO;
using System.Linq;
using Unity.Burst;
using Unity.Jobs.LowLevel.Unsafe;
using UnityEditor;
using UnityEngine;

namespace AraIteration
{
    public static class MacBatch
    {
        [Serializable]
        public sealed class PackageRow
        {
            public string name, version, source;
        }

        [Serializable]
        public sealed class PreferenceRow
        {
            public string key;
            public bool existed, value;
        }

        [Serializable]
        public sealed class EnvironmentRow
        {
            public string lane, unity_version, editor_contents, processor;
            public int logical_processors, requested_threads, job_workers, job_worker_maximum;
            public bool burst_enabled, burst_safety_checks, caller_joins_jobs;
            public bool initial_burst_compilation, initial_burst_safety_checks;
            public bool initial_burst_force_safety_checks, initial_burst_debug;
            public bool initial_burst_compile_synchronously;
            public bool burst_force_safety_checks, burst_debug;
            public bool measurement_completed, option_values_restored, editor_preferences_restored, finally_restored;
            public PreferenceRow[] initial_editor_preferences;
            public PackageRow[] packages;
        }

        // Burst 1.8.29 writes all five keys whenever any global option changes.
        static PreferenceRow[] CapturePreferences() => new[]
        {
            "BurstCompilation", "BurstCompileSynchronously", "BurstShowTimings", "BurstDebug", "BurstForceSafetyChecks"
        }.Select(key => new PreferenceRow
        {
            key = key, existed = EditorPrefs.HasKey(key), value = EditorPrefs.GetBool(key, false)
        }).ToArray();

        static void RestorePreferences(PreferenceRow[] preferences)
        {
            foreach (var preference in preferences)
            {
                if (preference.existed) EditorPrefs.SetBool(preference.key, preference.value);
                else EditorPrefs.DeleteKey(preference.key);
            }
        }

        static bool PreferencesMatch(PreferenceRow[] preferences) => preferences.All(preference =>
            EditorPrefs.HasKey(preference.key) == preference.existed &&
            (!preference.existed || EditorPrefs.GetBool(preference.key) == preference.value));

        public static void Run()
        {
            if (!Application.isBatchMode)
            {
                Debug.LogError("CPU probe requires batch mode");
                return;
            }
            var options = BurstCompiler.Options;
            bool initialCompilation = options.EnableBurstCompilation;
            bool initialSafetyChecks = options.EnableBurstSafetyChecks;
            bool initialForceSafetyChecks = options.ForceEnableBurstSafetyChecks;
            bool initialDebug = options.EnableBurstDebug;
            bool initialSynchronous = options.EnableBurstCompileSynchronously;
            var initialPreferences = CapturePreferences();
            EnvironmentRow environment = null;
            string environmentPath = null;
            int exitCode = 1;
            try
            {
                var args = Environment.GetCommandLineArgs();
                int at = Array.IndexOf(args, "-s8") + 1;
                if (at == 0 || args.Length < at + 7) throw new ArgumentException("missing S8 arguments");
                environmentPath = args[at + 5] + ".environment.json";
                environment = new EnvironmentRow
                {
                    lane = "unity-editor-burst",
                    unity_version = Application.unityVersion,
                    editor_contents = EditorApplication.applicationContentsPath,
                    processor = SystemInfo.processorType,
                    logical_processors = Environment.ProcessorCount,
                    requested_threads = int.Parse(args[at + 2]),
                    job_worker_maximum = JobsUtility.JobWorkerMaximumCount,
                    initial_burst_compilation = initialCompilation,
                    initial_burst_safety_checks = initialSafetyChecks,
                    initial_burst_force_safety_checks = initialForceSafetyChecks,
                    initial_burst_debug = initialDebug,
                    initial_burst_compile_synchronously = initialSynchronous,
                    initial_editor_preferences = initialPreferences,
                    caller_joins_jobs = true,
                    packages = UnityEditor.PackageManager.PackageInfo.GetAllRegisteredPackages()
                        .OrderBy(p => p.name, StringComparer.Ordinal)
                        .Select(p => new PackageRow { name = p.name, version = p.version, source = p.source.ToString() })
                        .ToArray(),
                };
                options.EnableBurstCompilation = true;
                options.EnableBurstSafetyChecks = false;
                options.ForceEnableBurstSafetyChecks = false;
                options.EnableBurstDebug = false;
                environment.burst_enabled = BurstCompiler.IsEnabled;
                environment.burst_safety_checks = options.EnableBurstSafetyChecks;
                environment.burst_force_safety_checks = options.ForceEnableBurstSafetyChecks;
                environment.burst_debug = options.EnableBurstDebug;
                if (!environment.burst_enabled || environment.burst_safety_checks ||
                    environment.burst_force_safety_checks || environment.burst_debug)
                    throw new InvalidOperationException("Burst options differ from the controlled benchmark lane");
                S8Unity.Benchmark.Run();
                environment.job_workers = JobsUtility.JobWorkerCount;
                environment.measurement_completed = true;
                exitCode = 0;
            }
            catch (Exception error)
            {
                Debug.LogException(error);
            }
            finally
            {
                try
                {
                    // Burst synchronizes global options into EditorPrefs/SessionState.
                    // Attempt every restoration even if an earlier setter throws.
                    try { options.EnableBurstCompilation = initialCompilation; }
                    finally
                    {
                        try { options.EnableBurstSafetyChecks = initialSafetyChecks; }
                        finally
                        {
                            try { options.ForceEnableBurstSafetyChecks = initialForceSafetyChecks; }
                            finally { options.EnableBurstDebug = initialDebug; }
                        }
                    }
                }
                catch (Exception error)
                {
                    Debug.LogException(error);
                    exitCode = 1;
                }
                finally
                {
                    try
                    {
                        // Option callbacks can create absent keys; restore their known initial presence too.
                        RestorePreferences(initialPreferences);
                        if (environment != null)
                        {
                            environment.option_values_restored =
                                options.EnableBurstCompilation == initialCompilation &&
                                options.EnableBurstSafetyChecks == initialSafetyChecks &&
                                options.ForceEnableBurstSafetyChecks == initialForceSafetyChecks &&
                                options.EnableBurstDebug == initialDebug &&
                                options.EnableBurstCompileSynchronously == initialSynchronous;
                            environment.editor_preferences_restored = PreferencesMatch(initialPreferences);
                            environment.finally_restored = environment.option_values_restored &&
                                environment.editor_preferences_restored;
                            if (!environment.finally_restored) exitCode = 1;
                            File.WriteAllText(environmentPath, JsonUtility.ToJson(environment, true));
                        }
                    }
                    catch (Exception error)
                    {
                        Debug.LogException(error);
                        exitCode = 1;
                    }
                }
            }
            EditorApplication.Exit(exitCode);
        }
    }
}
