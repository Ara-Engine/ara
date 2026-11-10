using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using Unity.Collections;
using Unity.Collections.LowLevel.Unsafe;
using Unity.Entities;

namespace AraExtended
{
    /// <summary>Actual unmanaged struct layout and Entities chunk stride are different facts.</summary>
    [Serializable]
    public sealed class ComponentLayout
    {
        /// <summary>Component's native type name.</summary>
        public string name;
        /// <summary>Actual unmanaged struct size/alignment and installed Entities chunk stride.</summary>
        public int bytes, alignment, size_in_chunk;
    }
    /// <summary>Post-run storage observations; capacity is not an allocation counter.</summary>
    [Serializable]
    public sealed class Layout
    {
        /// <summary>Installed Entities TypeManager's actual buffer layout facts.</summary>
        public int inline_words, buffer_size_in_chunk, element_bytes, alignment;
        /// <summary>Observed capacities and associated entity counts.</summary>
        public int[] capacities, capacity_counts;
        /// <summary>Live words and buffers whose capacity exceeds inline capacity.</summary>
        public long live_words;
        /// <summary>External-capacity observations, not a pointer or allocator trace.</summary>
        public int capacity_above_inline;
        /// <summary>Actual layouts of components present in this case.</summary>
        public ComponentLayout[] components;
        /// <summary>Selected resized buffers passed capacity-policy assertions.</summary>
        public bool resize_storage_policy_checked;
    }

    /// <summary>Full identities, component presence, explicit epochs, stages and live buffers.</summary>
    public static class StateWriter
    {
        static ComponentLayout Component<T>() where T : unmanaged
        {
            return new ComponentLayout { name = typeof(T).Name, bytes = UnsafeUtility.SizeOf<T>(),
                alignment = UnsafeUtility.AlignOf<T>(), size_in_chunk = TypeManager.GetTypeInfo<T>().SizeInChunk };
        }
        /// <summary>Audits every logical row and exports the complete version-one state outside timing.</summary>
        public static long Write(EntityManager manager, string path, string name, int caseId, int count, uint steps, Config config, out Layout layout)
        {
            using var query = manager.CreateEntityQuery(typeof(Id), typeof(Value));
            if (query.CalculateEntityCount() != count) throw new InvalidOperationException("live count differs");
            using var actual = query.ToEntityArray(Allocator.Temp);
            var entities = new Entity[count]; var seen = new bool[count];
            foreach (Entity entity in actual)
            {
                uint id = manager.GetComponentData<Id>(entity).Value;
                if (id >= count || seen[(int)id]) throw new InvalidOperationException("duplicate/out-of-range ID");
                seen[(int)id] = true; entities[(int)id] = entity;
            }
            var histogram = new SortedDictionary<int, int>();
            var info = TypeManager.GetTypeInfo<Word>();
            layout = new Layout { inline_words = info.BufferCapacity, buffer_size_in_chunk = info.SizeInChunk,
                element_bytes = info.ElementSize, alignment = info.AlignmentInBytes };
            var components = new List<ComponentLayout> { Component<Id>(), Component<Value>() };
            if (name == "dirty_epoch") components.Add(Component<Epoch>());
            if (name == "archetype_churn") components.Add(Component<TagD>());
            if (name == "dependency_dag") { components.Add(Component<StageA>()); components.Add(Component<StageB>()); components.Add(Component<StageC>()); }
            if (name.StartsWith("buffer_", StringComparison.Ordinal)) components.Add(Component<Word>());
            layout.components = components.ToArray();
            layout.resize_storage_policy_checked = name == "buffer_resize" && steps >= 2;
            using var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write);
            using var writer = new BinaryWriter(stream);
            writer.Write(Encoding.ASCII.GetBytes("ARAECSF1")); writer.Write(1u); writer.Write((uint)caseId); writer.Write((uint)count); writer.Write(steps);
            for (uint id = 0; id < count; id++)
            {
                if (!seen[(int)id]) throw new InvalidOperationException("missing stable ID");
                Entity entity = entities[(int)id]; Value value = manager.GetComponentData<Value>(entity);
                bool dirty = name == "dirty_epoch", dag = name == "dependency_dag", buffers = name.StartsWith("buffer_", StringComparison.Ordinal);
                if (manager.HasComponent<Epoch>(entity) != dirty || manager.HasComponent<StageA>(entity) != dag
                    || manager.HasComponent<StageB>(entity) != dag || manager.HasComponent<StageC>(entity) != dag || manager.HasBuffer<Word>(entity) != buffers)
                    throw new InvalidOperationException("component presence differs");
                uint epoch = dirty ? manager.GetComponentData<Epoch>(entity).Value : 0;
                uint tags = manager.HasComponent<TagD>(entity) ? 8u : 0u;
                uint expectedUpdates = steps, expectedEpoch = 0;
                if (dirty)
                {
                    expectedUpdates = 0;
                    for (uint tick = 0; tick < steps; tick++)
                        if (MathContract.Selected(id, tick, (uint)count, (uint)config.percent, config.distribution == "clustered")) { expectedUpdates++; expectedEpoch = tick + 1; }
                }
                else if (name == "archetype_churn") expectedUpdates = id % 100 < config.percent ? (steps + 1) / 2 : 0;
                uint expectedTags = name == "archetype_churn" && steps > 0 && (steps - 1) % 2 == 0 && id % 100 < config.percent ? 8u : 0u;
                if (epoch != expectedEpoch || tags != expectedTags || value.Updates != expectedUpdates) throw new InvalidOperationException("epoch/tag/update audit failed");
                int length = 0;
                if (buffers)
                {
                    var words = manager.GetBuffer<Word>(entity, true); length = words.Length;
                    int expectedLength = name == "buffer_fixed" ? config.length : steps > 0 && (steps - 1) % 2 == 0 && id % 100 < config.percent ? 64 : 4;
                    if (length != expectedLength) throw new InvalidOperationException("buffer live length differs");
                    if (name == "buffer_resize" && steps >= 2 && id % 100 < config.percent)
                    {
                        bool shortTrim = steps % 2 == 0 && config.trim;
                        if (shortTrim ? words.Capacity != info.BufferCapacity : words.Capacity < 64)
                            throw new InvalidOperationException("resized buffer capacity policy differs");
                    }
                    histogram.TryGetValue(words.Capacity, out int hits); histogram[words.Capacity] = hits + 1;
                    layout.live_words += length; if (words.Capacity > info.BufferCapacity) layout.capacity_above_inline++;
                }
                writer.Write(id); writer.Write(tags); writer.Write(value.Word); writer.Write(epoch);
                writer.Write(dag ? manager.GetComponentData<StageA>(entity).Value : 0u);
                writer.Write(dag ? manager.GetComponentData<StageB>(entity).Value : 0u);
                writer.Write(dag ? manager.GetComponentData<StageC>(entity).Value : 0u);
                writer.Write(value.Updates); writer.Write((uint)length);
                if (buffers) { var words = manager.GetBuffer<Word>(entity, true); for (int index = 0; index < length; index++) writer.Write(words[index].Value); }
            }
            layout.capacities = new int[histogram.Count]; layout.capacity_counts = new int[histogram.Count];
            int slot = 0; foreach (var entry in histogram) { layout.capacities[slot] = entry.Key; layout.capacity_counts[slot++] = entry.Value; }
            writer.Flush(); return stream.Length;
        }
    }
}
