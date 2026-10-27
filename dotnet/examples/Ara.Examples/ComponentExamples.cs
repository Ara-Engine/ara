using System.Numerics;
using Ara;

namespace Ara.Examples;

/// <summary>Fixed-tick gameplay recipes. The target carries the named component.</summary>
public static class ComponentExamples
{
    #region types/AcousticPortal
    /// <summary>Leave a doorway partly open for sound transmission.</summary>
    public static void UseAcousticPortal(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<AcousticPortal>(entity);
        component.Open = 0.75f;
    }
    #endregion

    #region types/AcousticSource
    /// <summary>Reduce pitch shift on a moving emitter.</summary>
    public static void UseAcousticSource(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<AcousticSource>(entity);
        component.Doppler = 0.5f;
    }
    #endregion

    #region types/AcousticSurface
    /// <summary>Give a wall concrete acoustic absorption.</summary>
    public static void UseAcousticSurface(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<AcousticSurface>(entity);
        component.Preset = MaterialPreset.Concrete;
    }
    #endregion

    #region types/Animator
    /// <summary>Blend between idle and locomotion over 300 ms.</summary>
    public static void UseAnimator(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Animator>(entity);
        component.Fade = 0.3f;
    }
    #endregion

    #region types/AudioGraphSource
    /// <summary>Tune a procedural oscillator.</summary>
    public static void UseAudioGraphSource(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<AudioGraphSource>(entity);
        component.Parameters = new() { ["frequency"] = 440f };
    }
    #endregion

    #region types/AudioListener
    /// <summary>Place the listener at ear height.</summary>
    public static void UseAudioListener(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<AudioListener>(entity);
        component.Position = new Vector3(0, 1.7f, 0);
    }
    #endregion

    #region types/AudioSource
    /// <summary>Play an alarm through an AudioSource; the presentation layer adopts the clip.</summary>
    public static void UseAudioSource(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<AudioSource>(entity);
        component.Clip = "@audio/alarm.wav#as_alarm";
        component.Gain = 0.4f;
    }
    #endregion

    #region types/Brain
    /// <summary>Pause an NPC brain during a cutscene.</summary>
    public static void UseBrain(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Brain>(entity);
        component.Enabled = false;
    }
    #endregion

    #region types/Camera
    /// <summary>Widen the field of view while sprinting.</summary>
    public static void UseCamera(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Camera>(entity);
        component.Fov = 75f;
    }
    #endregion

    #region types/Camera2d
    /// <summary>Zoom in on a 2D room.</summary>
    public static void UseCamera2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Camera2d>(entity);
        component.Zoom = 1.5f;
    }
    #endregion

    #region types/CharacterController
    /// <summary>Allow a character to climb a shallow stair.</summary>
    public static void UseCharacterController(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<CharacterController>(entity);
        component.StepHeight = 0.4f;
    }
    #endregion

    #region types/CharacterController2d
    /// <summary>Tune a platform character for shallow steps.</summary>
    public static void UseCharacterController2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<CharacterController2d>(entity);
        component.StepHeight = 0.3f;
    }
    #endregion

    #region types/CharacterInput
    /// <summary>Move a character along the ground this fixed tick.</summary>
    public static void UseCharacterInput(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<CharacterInput>(entity);
        component.DesiredVelocity = new Vector3(2, 0, 0);
    }
    #endregion

    #region types/CharacterInput2d
    /// <summary>Request a platform jump this fixed tick.</summary>
    public static void UseCharacterInput2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<CharacterInput2d>(entity);
        component.JumpSpeed = 6f;
    }
    #endregion

    #region types/CharacterMotion
    /// <summary>Restore grounded state when resetting a character on the floor.</summary>
    public static void UseCharacterMotion(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<CharacterMotion>(entity);
        component.Grounded = true;
    }
    #endregion

    #region types/CharacterMotion2d
    /// <summary>Restore grounded state when resetting a platform character on the floor.</summary>
    public static void UseCharacterMotion2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<CharacterMotion2d>(entity);
        component.Grounded = true;
    }
    #endregion

    #region types/Collider
    /// <summary>Give a crate a grippy surface.</summary>
    public static void UseCollider(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Collider>(entity);
        component.Friction = 0.8f;
    }
    #endregion

    #region types/Collider2d
    /// <summary>Make a 2D ball bounce.</summary>
    public static void UseCollider2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Collider2d>(entity);
        component.Restitution = 0.6f;
    }
    #endregion

    #region types/Environment
    /// <summary>Brighten a scene during a daylight transition.</summary>
    public static void UseEnvironment(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Ara.Environment>(entity);
        component.Exposure = 0.0025f;
    }
    #endregion

    #region types/Flipbook
    /// <summary>Loop a campfire flipbook while the fire is burning.</summary>
    public static void UseFlipbook(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Flipbook>(entity);
        component.Looping = true;
    }
    #endregion

    #region types/FollowCamera
    /// <summary>Pull a following camera back for a vehicle.</summary>
    public static void UseFollowCamera(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<FollowCamera>(entity);
        component.Distance = 6f;
    }
    #endregion

    #region types/HeightFog
    /// <summary>Thicken ground fog as dusk approaches.</summary>
    public static void UseHeightFog(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<HeightFog>(entity);
        component.Density = 0.03f;
    }
    #endregion

    #region types/Joint
    /// <summary>Move a hinge anchor to the top of a panel.</summary>
    public static void UseJoint(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Joint>(entity);
        component.AnchorA = new Vector3(0, 0.5f, 0);
    }
    #endregion

    #region types/Light
    /// <summary>Flicker a lamp using fixed tick time, without wall-clock randomness.</summary>
    public static void UseLight(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Light>(entity);
        component.Intensity = tick.Frame % 4 == 0 ? 450f : 500f;
    }
    #endregion

    #region types/Light2d
    /// <summary>Extend a 2D lantern radius.</summary>
    public static void UseLight2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Light2d>(entity);
        component.Radius = 8f;
    }
    #endregion

    #region types/MeshRenderer
    /// <summary>Brighten a model lamp without replacing its material.</summary>
    public static void UseMeshRenderer(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<MeshRenderer>(entity);
        component.EmissiveStrength = 2f;
    }
    #endregion

    #region types/MusicSource
    /// <summary>Raise adaptive music intensity during combat.</summary>
    public static void UseMusicSource(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<MusicSource>(entity);
        component.Intensity = 0.8f;
    }
    #endregion

    #region types/NavAgent
    /// <summary>Let an alerted NPC run toward its goal.</summary>
    public static void UseNavAgent(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<NavAgent>(entity);
        component.MaxSpeed = 4f;
    }
    #endregion

    #region types/NavAgentType
    /// <summary>Configure a navigation profile for low steps.</summary>
    public static void UseNavAgentType(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<NavAgentType>(entity);
        component.Climb = 0.5f;
    }
    #endregion

    #region types/NavArea
    /// <summary>Make agents prefer routes around a hazardous area.</summary>
    public static void UseNavArea(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<NavArea>(entity);
        component.Cost = 3f;
    }
    #endregion

    #region types/NavLink
    /// <summary>Set the traversal time of a jump link.</summary>
    public static void UseNavLink(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<NavLink>(entity);
        component.Duration = 0.6f;
    }
    #endregion

    #region types/NavMeshSurface
    /// <summary>Choose a finer navigation bake cell size.</summary>
    public static void UseNavMeshSurface(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<NavMeshSurface>(entity);
        component.CellSize = 0.25f;
    }
    #endregion

    #region types/NavObstacle
    /// <summary>Resize a dynamic carving obstacle around a crate.</summary>
    public static void UseNavObstacle(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<NavObstacle>(entity);
        component.HalfExtents = new Vector3(1, 1, 1);
    }
    #endregion

    #region types/Perceiver
    /// <summary>Give a lookout a longer sight range.</summary>
    public static void UsePerceiver(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Perceiver>(entity);
        component.SightRange = 30f;
    }
    #endregion

    #region types/PhysicsId
    /// <summary>Restore a stable physics identity from a checkpoint.</summary>
    public static void UsePhysicsId(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<PhysicsId>(entity);
        component.Value = 42;
    }
    #endregion

    #region types/PhysicsPose
    /// <summary>Reset a body pose above the floor.</summary>
    public static void UsePhysicsPose(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<PhysicsPose>(entity);
        component.Position = new Vector3(0, 2, 0);
    }
    #endregion

    #region types/PhysicsPose2d
    /// <summary>Reset a platform body above the floor.</summary>
    public static void UsePhysicsPose2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<PhysicsPose2d>(entity);
        component.Position = new Vector2(0, 2);
    }
    #endregion

    #region types/PlayerControl
    /// <summary>Increase the player sprint speed.</summary>
    public static void UsePlayerControl(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<PlayerControl>(entity);
        component.RunSpeed = 7f;
    }
    #endregion

    #region types/PostProcessVolume
    /// <summary>Call each fixed tick with a room trigger's occupancy to fade its camera treatment in and out.</summary>
    public static void UsePostProcessVolume(Entity entity, bool playerInside, in Tick tick)
    {
        ref var volume = ref World.Get<PostProcessVolume>(entity);
        volume.Shape = VolumeShape.Box;
        volume.BlendDistance = 2f;
        volume.Weight = Math.Clamp(volume.Weight + (playerInside ? 2f : -2f) * tick.Dt, 0f, 1f);
        volume.Overrides = [
            new PostOverride {
                Module = PostModule.ColorGrading, WhiteBalance = 4500f, Saturation = 0.8f,
                Lut = "@textures/security_lut.png#as_alarm", LutContribution = 0.6f
            },
            new PostOverride {
                Module = PostModule.DepthOfField, FocusDistance = 8f, Aperture = 2.8f,
                NearBlur = 0.5f, FarBlur = 1f
            },
            new PostOverride { Module = PostModule.MotionBlur, ShutterAngle = 180f }
        ];
    }
    #endregion

    #region types/ReverbZone
    /// <summary>Increase the reverb blend inside a large room.</summary>
    public static void UseReverbZone(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<ReverbZone>(entity);
        component.Wet = 0.6f;
    }
    #endregion

    #region types/RigidBody
    /// <summary>Give a salvage crate a heavier physical mass.</summary>
    public static void UseRigidBody(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<RigidBody>(entity);
        component.Mass = 20f;
    }
    #endregion

    #region types/RigidBody2d
    /// <summary>Give a 2D pickup a small physical mass.</summary>
    public static void UseRigidBody2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<RigidBody2d>(entity);
        component.Mass = 2f;
    }
    #endregion

    #region types/SkyAtmosphere
    /// <summary>Bring in clouds during a weather transition.</summary>
    public static void UseSkyAtmosphere(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<SkyAtmosphere>(entity);
        component.CloudCoverage = 0.7f;
    }
    #endregion

    #region types/Sprite
    /// <summary>Face a sprite toward leftward movement.</summary>
    public static void UseSprite(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Sprite>(entity);
        component.FlipX = true;
    }
    #endregion

    #region types/Stimulus
    /// <summary>Reduce a crouched target visibility to AI.</summary>
    public static void UseStimulus(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Stimulus>(entity);
        component.Visibility = 0.25f;
    }
    #endregion

    #region types/StreamedWorld
    /// <summary>Select a streamed world for an expedition.</summary>
    public static void UseStreamedWorld(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<StreamedWorld>(entity);
        component.World = "@levels/island.world#wo_island";
    }
    #endregion

    #region types/Terrain
    /// <summary>Bind the terrain asset used by a world cell.</summary>
    public static void UseTerrain(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Terrain>(entity);
        component.TerrainAsset = "@terrain/island.terrain#te_island";
    }
    #endregion

    #region types/TerrainChunk
    /// <summary>Place a terrain chunk in the next grid column.</summary>
    public static void UseTerrainChunk(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<TerrainChunk>(entity);
        component.X = 1;
    }
    #endregion

    #region types/TileColliderSource2d
    /// <summary>Associate generated tile collision with its source tilemap.</summary>
    public static void UseTileColliderSource2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<TileColliderSource2d>(entity);
        component.Owner = "e_tilemap";
    }
    #endregion

    #region types/Tilemap
    /// <summary>Use two-metre cells for a large tilemap.</summary>
    public static void UseTilemap(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Tilemap>(entity);
        component.CellSize = new Vector2(2, 2);
    }
    #endregion

    #region types/Transform
    /// <summary>Move an entity upwards at one metre per second.</summary>
    public static void UseTransform(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Transform>(entity);
        component.Position += new Vector3(0, tick.Dt, 0);
    }
    #endregion

    #region types/Vehicle
    /// <summary>Increase a vehicle engine torque.</summary>
    public static void UseVehicle(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Vehicle>(entity);
        component.EngineMaxTorque = 600f;
    }
    #endregion

    #region types/VehicleInput
    /// <summary>Apply half throttle this fixed tick.</summary>
    public static void UseVehicleInput(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<VehicleInput>(entity);
        component.Throttle = 0.5f;
    }
    #endregion

    #region types/VehicleTelemetry
    /// <summary>Restore an idling engine telemetry sample.</summary>
    public static void UseVehicleTelemetry(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<VehicleTelemetry>(entity);
        component.EngineRpm = 1200f;
    }
    #endregion

    #region types/Velocity
    /// <summary>Apply a two-metre-per-second upward velocity impulse.</summary>
    public static void UseVelocity(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Velocity>(entity);
        component.Linear += new Vector3(0, 2, 0);
    }
    #endregion

    #region types/Velocity2d
    /// <summary>Apply a horizontal 2D velocity impulse.</summary>
    public static void UseVelocity2d(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Velocity2d>(entity);
        component.Linear += new Vector2(2, 0);
    }
    #endregion

    #region types/VisualEffect
    /// <summary>Reduce a running particle effect intensity.</summary>
    public static void UseVisualEffect(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<VisualEffect>(entity);
        component.Intensity = 0.5f;
    }
    #endregion

    #region types/Wanderer
    /// <summary>Let a wandering NPC pause for two seconds at 60 Hz.</summary>
    public static void UseWanderer(Entity entity, in Tick tick)
    {
        ref var component = ref World.Get<Wanderer>(entity);
        component.PauseTicks = 120;
    }
    #endregion

    /// <summary>Executes every recipe against an entity carrying all engine components.</summary>
    public static void RunAll(Entity entity, in Tick tick)
    {
        UseAcousticPortal(entity, in tick);
        UseAcousticSource(entity, in tick);
        UseAcousticSurface(entity, in tick);
        UseAnimator(entity, in tick);
        UseAudioGraphSource(entity, in tick);
        UseAudioListener(entity, in tick);
        UseAudioSource(entity, in tick);
        UseBrain(entity, in tick);
        UseCamera(entity, in tick);
        UseCamera2d(entity, in tick);
        UseCharacterController(entity, in tick);
        UseCharacterController2d(entity, in tick);
        UseCharacterInput(entity, in tick);
        UseCharacterInput2d(entity, in tick);
        UseCharacterMotion(entity, in tick);
        UseCharacterMotion2d(entity, in tick);
        UseCollider(entity, in tick);
        UseCollider2d(entity, in tick);
        UseEnvironment(entity, in tick);
        UseFlipbook(entity, in tick);
        UseFollowCamera(entity, in tick);
        UseHeightFog(entity, in tick);
        UseJoint(entity, in tick);
        UseLight(entity, in tick);
        UseLight2d(entity, in tick);
        UseMeshRenderer(entity, in tick);
        UseMusicSource(entity, in tick);
        UseNavAgent(entity, in tick);
        UseNavAgentType(entity, in tick);
        UseNavArea(entity, in tick);
        UseNavLink(entity, in tick);
        UseNavMeshSurface(entity, in tick);
        UseNavObstacle(entity, in tick);
        UsePerceiver(entity, in tick);
        UsePhysicsId(entity, in tick);
        UsePhysicsPose(entity, in tick);
        UsePhysicsPose2d(entity, in tick);
        UsePlayerControl(entity, in tick);
        UsePostProcessVolume(entity, playerInside: true, in tick);
        UseReverbZone(entity, in tick);
        UseRigidBody(entity, in tick);
        UseRigidBody2d(entity, in tick);
        UseSkyAtmosphere(entity, in tick);
        UseSprite(entity, in tick);
        UseStimulus(entity, in tick);
        UseStreamedWorld(entity, in tick);
        UseTerrain(entity, in tick);
        UseTerrainChunk(entity, in tick);
        UseTileColliderSource2d(entity, in tick);
        UseTilemap(entity, in tick);
        UseTransform(entity, in tick);
        UseVehicle(entity, in tick);
        UseVehicleInput(entity, in tick);
        UseVehicleTelemetry(entity, in tick);
        UseVelocity(entity, in tick);
        UseVelocity2d(entity, in tick);
        UseVisualEffect(entity, in tick);
        UseWanderer(entity, in tick);
    }
}
