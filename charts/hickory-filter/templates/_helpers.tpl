{{- define "hickory-filter.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{- define "hickory-filter.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{- define "hickory-filter.selectorLabels" -}}
app.kubernetes.io/name: {{ include "hickory-filter.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{- define "hickory-filter.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{ include "hickory-filter.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}

{{- define "hickory-filter.serviceAccountName" -}}
{{- if .Values.serviceAccount.create }}
{{- default (include "hickory-filter.fullname" .) .Values.serviceAccount.name }}
{{- else }}
{{- default "default" .Values.serviceAccount.name }}
{{- end }}
{{- end }}

{{- define "hickory-filter.image" -}}
{{- if .Values.image.digest }}
{{- printf "%s@%s" .Values.image.repository .Values.image.digest }}
{{- else }}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) }}
{{- end }}
{{- end }}

{{/* The HTTP port from config.http: "" when the endpoint is disabled. */}}
{{- define "hickory-filter.httpPort" -}}
{{- $http := "0.0.0.0:8080" -}}
{{- if hasKey .Values.config "http" }}{{ $http = .Values.config.http }}{{ end -}}
{{- if kindIs "string" $http }}{{ last (splitList ":" $http) }}{{ end -}}
{{- end }}

{{/* Fails when a feature needs the HTTP endpoint and the config disables it. */}}
{{- define "hickory-filter.requireHttp" -}}
{{- if not (include "hickory-filter.httpPort" .) }}
{{- fail (printf "%s is enabled but the metrics endpoint is disabled (config.http is false)" .feature) }}
{{- end }}
{{- end }}

{{/* The rendered configuration, with the cache directory filled in. */}}
{{- define "hickory-filter.config" -}}
{{- $cfg := deepCopy .Values.config -}}
{{- /* Recursive mode takes no upstreams; the default values carry some for forward mode. */ -}}
{{- if eq (toString (get $cfg "mode")) "recursive" }}
{{- $_ := unset $cfg "upstreams" }}
{{- end }}
{{- if .Values.cache.enabled }}
{{- $filter := default (dict) (get $cfg "filter") }}
{{- $_ := set $filter "cache_dir" .Values.cache.mountPath }}
{{- $_ := set $cfg "filter" $filter }}
{{- end }}
{{- toToml $cfg }}
{{- end }}

{{- define "hickory-filter.configName" -}}
{{- default (include "hickory-filter.fullname" .) .Values.existingConfigMap }}
{{- end }}
