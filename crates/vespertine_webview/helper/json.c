/* json-glib helpers: defaulting accessors (PLT-015 R1) and builder shortcuts. */
#include "vsp.h"

/* Parses a JSON object. Returns NULL for invalid JSON or non-object payloads. The returned object
 * is owned by *parser_out, which the caller must unref. */
JsonObject *vsp_json_parse_object(const char *data, gsize len, JsonParser **parser_out)
{
    *parser_out = NULL;
    if (len == 0)
        return NULL;
    JsonParser *parser = json_parser_new();
    if (!json_parser_load_from_data(parser, data, (gssize)len, NULL)) {
        g_object_unref(parser);
        return NULL;
    }
    JsonNode *root = json_parser_get_root(parser);
    if (!root || json_node_get_node_type(root) != JSON_NODE_OBJECT) {
        g_object_unref(parser);
        return NULL;
    }
    *parser_out = parser;
    return json_node_get_object(root);
}

static JsonNode *member_value(JsonObject *obj, const char *name)
{
    if (!obj || !json_object_has_member(obj, name))
        return NULL;
    JsonNode *node = json_object_get_member(obj, name);
    if (!node || json_node_get_node_type(node) != JSON_NODE_VALUE)
        return NULL;
    return node;
}

const char *vsp_json_string(JsonObject *obj, const char *name, const char *fallback)
{
    JsonNode *node = member_value(obj, name);
    if (!node || json_node_get_value_type(node) != G_TYPE_STRING)
        return fallback;
    const char *value = json_node_get_string(node);
    return value ? value : fallback;
}

double vsp_json_double(JsonObject *obj, const char *name, double fallback)
{
    JsonNode *node = member_value(obj, name);
    if (!node)
        return fallback;
    GType type = json_node_get_value_type(node);
    if (type == G_TYPE_DOUBLE || type == G_TYPE_INT64)
        return json_node_get_double(node);
    return fallback;
}

gint64 vsp_json_int(JsonObject *obj, const char *name, gint64 fallback)
{
    JsonNode *node = member_value(obj, name);
    if (!node)
        return fallback;
    GType type = json_node_get_value_type(node);
    if (type == G_TYPE_INT64)
        return json_node_get_int(node);
    if (type == G_TYPE_DOUBLE) {
        double d = json_node_get_double(node);
        if (d != d)
            return fallback;
        if (d >= 9.2e18)
            return G_MAXINT64;
        if (d <= -9.2e18)
            return G_MININT64;
        return (gint64)d;
    }
    return fallback;
}

/* Booleans accept JSON booleans and numbers (nonzero is true). */
gboolean vsp_json_bool(JsonObject *obj, const char *name, gboolean fallback)
{
    JsonNode *node = member_value(obj, name);
    if (!node)
        return fallback;
    GType type = json_node_get_value_type(node);
    if (type == G_TYPE_BOOLEAN)
        return json_node_get_boolean(node);
    if (type == G_TYPE_INT64)
        return json_node_get_int(node) != 0;
    if (type == G_TYPE_DOUBLE)
        return json_node_get_double(node) != 0.0;
    return fallback;
}

JsonBuilder *vsp_json_begin(void)
{
    JsonBuilder *builder = json_builder_new();
    json_builder_begin_object(builder);
    return builder;
}

/* Closes the root object, serializes and consumes the builder. */
char *vsp_json_finish(JsonBuilder *builder, gsize *len)
{
    json_builder_end_object(builder);
    JsonNode *root = json_builder_get_root(builder);
    JsonGenerator *generator = json_generator_new();
    json_generator_set_root(generator, root);
    char *text = json_generator_to_data(generator, len);
    json_node_unref(root);
    g_object_unref(generator);
    g_object_unref(builder);
    return text;
}
