# HashiCorp Nomad job spec — an example, not a supported target. It exists to
# prove orchestrator-independence: tellurion is a static binary plus
# PostgreSQL, so an orchestrator that runs raw executables needs no OCI image,
# no sidecar and no platform-specific code path.
#
# The `exec` driver is the point of this file. The `docker` driver works too
# and is a two-line change (commented at the bottom of the task).
#
#   nomad job run \
#     -var artifact_source=https://artifacts.internal/tellurion-v0.5.0-rc.1-x86_64-unknown-linux-musl.tar.gz \
#     -var config_source=https://artifacts.internal/tellurion-config.tar.gz \
#     deploy/nomad/tellurion.nomad.hcl

variable "artifact_source" {
  type = string
  # No default on purpose: this repository publishes no download URL, so
  # inventing one here would be a lie. Point it at the release archive built
  # by .github/workflows/release-artifacts.yml, mirrored into your own
  # artifact store (the same mirror an air-gapped install already needs — see
  # deploy/airgap/images.txt).
  description = "URL of the tellurion release archive (tar.gz containing the tellurion binary)"
}

variable "config_source" {
  type        = string
  description = "URL of an archive containing config.yaml and any styles/ it references"
}

variable "datacenters" {
  type    = list(string)
  default = ["dc1"]
}

job "tellurion" {
  datacenters = var.datacenters
  type        = "service"

  update {
    max_parallel = 1
    # Nomad marks an allocation healthy from the service check below, i.e.
    # from /readyz — the same bounded-dependency readiness Kubernetes uses.
    health_check     = "checks"
    min_healthy_time = "10s"
    healthy_deadline = "2m"
    auto_revert      = true
  }

  group "server" {
    # Read serving is stateless and safe to fan out. Background outbox
    # consumers are not yet leader-elected, so see
    # docs/deployment-topologies.md §"Running more than one replica" before
    # raising this.
    count = 1

    network {
      port "http" {
        to = 8080
      }
    }

    service {
      name     = "tellurion"
      port     = "http"
      provider = "nomad"

      check {
        type     = "http"
        path     = "/readyz"
        interval = "10s"
        timeout  = "2s"
      }
    }

    task "tellurion" {
      driver = "exec"

      # SIGTERM starts the drain; the default 10s drain plus process shutdown
      # is the same budget as the Kubernetes base's 15s grace period.
      kill_signal  = "SIGTERM"
      kill_timeout = "15s"

      artifact {
        source      = var.artifact_source
        destination = "local/bin"
      }

      artifact {
        source      = var.config_source
        destination = "local/config"
      }

      config {
        # The release archive unpacks into a versioned directory; `**` lets one
        # spec survive a version bump without an extra variable.
        command = "local/bin/**/tellurion"
      }

      env {
        TELLURION_CONFIG = "local/config/config.yaml"
        PORT             = "8080"
      }

      # DATABASE_URL is a secret, so it comes from a Nomad Variable rather than
      # the job file. Create it once:
      #
      #   nomad var put nomad/jobs/tellurion database_url=postgres://...
      template {
        destination = "secrets/db.env"
        env         = true
        data        = <<-EOT
          {{- with nomadVar "nomad/jobs/tellurion" -}}
          DATABASE_URL={{ .database_url }}
          {{- end -}}
        EOT
      }

      resources {
        cpu    = 500
        memory = 512
      }

      # OCI alternative — replace `driver`, both `artifact` blocks and `config`
      # above with:
      #
      #   driver = "docker"
      #   config {
      #     image = "ghcr.io/ccancellieri/tellurion:latest"
      #     ports = ["http"]
      #   }
    }
  }
}
