# LightTrack on GCP Cloud Run + Secret Manager.
# UNVERIFIED template (authored without terraform to validate). Run `terraform validate` and
# `terraform plan` against your project before `apply`. Enable APIs first:
#   gcloud services enable run.googleapis.com secretmanager.googleapis.com

terraform {
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = ">= 5.0"
    }
  }
}

data "google_project" "this" {
  project_id = var.project_id
}

locals {
  # Cloud Run v2 uses the default compute service account unless overridden.
  run_sa = "serviceAccount:${data.google_project.this.number}-compute@developer.gserviceaccount.com"

  # Only create secrets/env for values that are actually provided.
  secret_env = merge(
    var.admin_key == "" ? {} : { LIGHTTRACK_ADMIN_KEY = var.admin_key },
    var.database_url == "" ? {} : { LIGHTTRACK_DATABASE_URL = var.database_url },
  )
}

resource "google_secret_manager_secret" "s" {
  for_each  = local.secret_env
  project   = var.project_id
  secret_id = "${var.name}-${lower(replace(each.key, "_", "-"))}"
  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "v" {
  for_each    = local.secret_env
  secret      = google_secret_manager_secret.s[each.key].id
  secret_data = each.value
}

resource "google_secret_manager_secret_iam_member" "access" {
  for_each  = local.secret_env
  secret_id = google_secret_manager_secret.s[each.key].id
  role      = "roles/secretmanager.secretAccessor"
  member    = local.run_sa
}

resource "google_cloud_run_v2_service" "this" {
  project  = var.project_id
  name     = var.name
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  template {
    scaling {
      min_instance_count = var.min_instances
      max_instance_count = var.max_instances
    }
    containers {
      image = var.image
      ports {
        container_port = 8080
      }
      env {
        name  = "LIGHTTRACK_BIND"
        value = "0.0.0.0:8080"
      }
      env {
        name  = "LIGHTTRACK_AUTH_MODE"
        value = var.auth_mode
      }
      env {
        name  = "LIGHTTRACK_DB"
        value = "/data/lighttrack.db"
      }
      dynamic "env" {
        for_each = local.secret_env
        content {
          name = env.key
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.s[env.key].secret_id
              version = "latest"
            }
          }
        }
      }
    }
  }

  depends_on = [
    google_secret_manager_secret_version.v,
    google_secret_manager_secret_iam_member.access,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "public" {
  count    = var.allow_public ? 1 : 0
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.this.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}
