using System;
using System.IO;
using System.Text;
using Unity.Collections;
using Unity.Entities;

namespace AraFeatures
{
    /// <summary>Writes and audits the complete logical feature state outside timing.</summary>
    public static class StateWriter
    {
        struct Record { public uint id, tags, value, updates; }

        /// <summary>Rejects missing/duplicate IDs, wrong tags or skipped updates, then exports every field.</summary>
        public static long Write(EntityManager manager, string path, int count, int archetypes, uint steps)
        {
            using var query = manager.CreateEntityQuery(typeof(StableId), typeof(FeatureValue));
            if (query.CalculateEntityCount() != count) throw new InvalidOperationException("live component count differs");
            using var entities = query.ToEntityArray(Allocator.Temp);
            using var ids = query.ToComponentDataArray<StableId>(Allocator.Temp);
            using var values = query.ToComponentDataArray<FeatureValue>(Allocator.Temp);
            var records = new Record[count];
            var seen = new bool[count];
            for (int i = 0; i < count; i++)
            {
                uint id = ids[i].Value;
                if (id >= (uint)count || seen[(int)id]) throw new InvalidOperationException("duplicate/out-of-range ID");
                seen[(int)id] = true;
                Entity entity = entities[i];
                uint tags = (manager.HasComponent<TagA>(entity) ? 1u : 0u)
                    | (manager.HasComponent<TagB>(entity) ? 2u : 0u)
                    | (manager.HasComponent<TagC>(entity) ? 4u : 0u)
                    | (manager.HasComponent<TagD>(entity) ? 8u : 0u)
                    | (manager.HasComponent<TagE>(entity) ? 16u : 0u)
                    | (manager.HasComponent<TagF>(entity) ? 32u : 0u);
                uint expectedUpdates = (tags & 1) != 0 && (tags & 2) == 0 ? steps : 0;
                if (tags != (id & (uint)(archetypes - 1)) || values[i].Updates != expectedUpdates)
                    throw new InvalidOperationException("actual tags or update count differ");
                records[(int)id] = new Record { id = id, tags = tags, value = values[i].Value, updates = values[i].Updates };
            }
            for (int i = 0; i < count; i++)
                if (!seen[i]) throw new InvalidOperationException("missing stable ID");
            using var stream = new FileStream(path, FileMode.CreateNew, FileAccess.Write);
            using var writer = new BinaryWriter(stream);
            writer.Write(Encoding.ASCII.GetBytes("ARAECSF1"));
            writer.Write(1u); writer.Write(1u); writer.Write((uint)count); writer.Write(steps);
            foreach (var record in records)
            {
                writer.Write(record.id); writer.Write(record.tags); writer.Write(record.value);
                writer.Write(0u); writer.Write(0u); writer.Write(0u); writer.Write(0u);
                writer.Write(record.updates); writer.Write(0u);
            }
            writer.Flush();
            return stream.Length;
        }
    }
}
