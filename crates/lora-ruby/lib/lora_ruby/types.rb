# frozen_string_literal: true

module LoraRuby
  # Shared value model — aligned with the `LoraValue` contract used by
  # `lora-node`, `lora-wasm`, and `lora-python`.
  #
  # - Scalars pass through as Ruby natives (`nil`, `true`, `false`,
  #   `Integer`, `Float`, `String`).
  # - Lists and maps come back as `Array` / `Hash` (string keys).
  # - Graph, temporal, and spatial values come back as plain `Hash`es
  #   with a `"kind"` discriminator.
  #
  # If you want to narrow a value explicitly, use the `node?` / `point?`
  # / `temporal?` helpers below.
  module Types
    SRID_CARTESIAN_2D  = 7203
    SRID_CARTESIAN_3D  = 9157
    SRID_WGS84_2D      = 4326
    SRID_WGS84_3D      = 4979

    CRS_CARTESIAN_2D   = "cartesian"
    CRS_CARTESIAN_3D   = "cartesian-3D"
    CRS_WGS84_2D       = "WGS-84-2D"
    CRS_WGS84_3D       = "WGS-84-3D"

    TEMPORAL_KINDS = %w[date time localtime datetime localdatetime duration].freeze

    module_function

    # ------------------------------------------------------------------
    # Temporal constructors — ISO-8601 tagged Hashes. The native
    # extension normalises + validates these on the way into the engine;
    # invalid ISO strings raise `LoraRuby::InvalidParamsError`.
    # ------------------------------------------------------------------

    def date(iso)          = { "kind" => "date",          "iso" => iso }
    def time(iso)          = { "kind" => "time",          "iso" => iso }
    def localtime(iso)     = { "kind" => "localtime",     "iso" => iso }
    def datetime(iso)      = { "kind" => "datetime",      "iso" => iso }
    def localdatetime(iso) = { "kind" => "localdatetime", "iso" => iso }
    def duration(iso)      = { "kind" => "duration",      "iso" => iso }

    # ------------------------------------------------------------------
    # Spatial constructors — mirrors lora_python.cartesian / wgs84.
    # `cartesian(1, 2)` returns a 2D cartesian point; use `cartesian_3d`
    # for the 3D variant. WGS-84 variants carry `longitude` / `latitude`
    # aliases alongside `x` / `y` so result-side consumers can read
    # either without conversion.
    # ------------------------------------------------------------------

    def cartesian(x, y)
      {
        "kind" => "point",
        "srid" => SRID_CARTESIAN_2D,
        "crs"  => CRS_CARTESIAN_2D,
        "x"    => x.to_f,
        "y"    => y.to_f,
      }
    end

    def cartesian_3d(x, y, z)
      {
        "kind" => "point",
        "srid" => SRID_CARTESIAN_3D,
        "crs"  => CRS_CARTESIAN_3D,
        "x"    => x.to_f,
        "y"    => y.to_f,
        "z"    => z.to_f,
      }
    end

    def wgs84(longitude, latitude)
      {
        "kind"      => "point",
        "srid"      => SRID_WGS84_2D,
        "crs"       => CRS_WGS84_2D,
        "x"         => longitude.to_f,
        "y"         => latitude.to_f,
        "longitude" => longitude.to_f,
        "latitude"  => latitude.to_f,
      }
    end

    def wgs84_3d(longitude, latitude, height)
      {
        "kind"      => "point",
        "srid"      => SRID_WGS84_3D,
        "crs"       => CRS_WGS84_3D,
        "x"         => longitude.to_f,
        "y"         => latitude.to_f,
        "z"         => height.to_f,
        "longitude" => longitude.to_f,
        "latitude"  => latitude.to_f,
        "height"    => height.to_f,
      }
    end

    # ------------------------------------------------------------------
    # Guards — duck-typed narrowing helpers. Accept symbol-keyed and
    # string-keyed Hashes because that's what callers might build up
    # manually (the native extension always emits string keys).
    # ------------------------------------------------------------------

    def node?(v)         = tagged?(v, "node")
    def relationship?(v) = tagged?(v, "relationship")
    def path?(v)         = tagged?(v, "path")
    def point?(v)        = tagged?(v, "point")

    def temporal?(v)
      return false unless v.is_a?(Hash)
      TEMPORAL_KINDS.include?(kind_of(v))
    end

    def tagged?(v, expected)
      return false unless v.is_a?(Hash)
      kind_of(v) == expected
    end

    def kind_of(hash)
      hash["kind"] || hash[:kind]
    end
  end
end
