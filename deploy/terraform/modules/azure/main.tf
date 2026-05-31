# LightTrack on Azure Container Apps (with built-in secrets + Log Analytics).
# UNVERIFIED template (authored without terraform to validate). Run `terraform validate` and
# `terraform plan` before `apply`. Requires the Microsoft.App provider registered on the subscription.

terraform {
  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = ">= 3.80"
    }
  }
}

resource "azurerm_log_analytics_workspace" "this" {
  name                = "${var.name}-logs"
  resource_group_name = var.resource_group_name
  location            = var.location
  sku                 = "PerGB2018"
  retention_in_days   = 30
}

resource "azurerm_container_app_environment" "this" {
  name                       = "${var.name}-env"
  resource_group_name        = var.resource_group_name
  location                   = var.location
  log_analytics_workspace_id = azurerm_log_analytics_workspace.this.id
}

locals {
  # secret_name => { env = ENV_VAR_NAME, value = secret_value }
  secrets = merge(
    var.admin_key == "" ? {} : { "admin-key" = { env = "LIGHTTRACK_ADMIN_KEY", value = var.admin_key } },
    var.database_url == "" ? {} : { "database-url" = { env = "LIGHTTRACK_DATABASE_URL", value = var.database_url } },
  )
}

resource "azurerm_container_app" "this" {
  name                         = var.name
  resource_group_name          = var.resource_group_name
  container_app_environment_id = azurerm_container_app_environment.this.id
  revision_mode                = "Single"

  dynamic "secret" {
    for_each = local.secrets
    content {
      name  = secret.key
      value = secret.value.value
    }
  }

  ingress {
    external_enabled = var.allow_public
    target_port      = 8787
    transport        = "auto"
    traffic_weight {
      latest_revision = true
      percentage      = 100
    }
  }

  template {
    min_replicas = var.min_replicas
    max_replicas = var.max_replicas

    container {
      name   = "api"
      image  = var.image
      cpu    = 0.25
      memory = "0.5Gi"

      env {
        name  = "LIGHTTRACK_BIND"
        value = "0.0.0.0:8787"
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
        for_each = local.secrets
        content {
          name        = env.value.env
          secret_name = env.key
        }
      }
    }
  }
}
