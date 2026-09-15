/* Struct definition test */
typedef struct {
    int x;
    int y;
    char *name;
} Point;

typedef struct {
    Point origin;
    Point corner;
    int width;
    int height;
} Rectangle;

/* Function that uses struct members */
int point_distance(Point *a, Point *b) {
    int dx = a->x - b->x;
    int dy = a->y - b->y;
    return dx * dx + dy * dy;
}

/* Function that modifies struct */
void point_set(Point *p, int x, int y) {
    p->x = x;
    p->y = y;
}

/* Struct with nested access */
int rect_area(Rectangle *rect) {
    return rect->width * rect->height;
}

/* Enum test */
typedef enum {
    COLOR_RED,
    COLOR_GREEN,
    COLOR_BLUE,
    COLOR_COUNT
} Color;

void print_color(Color c) {
    switch (c) {
        case COLOR_RED:
            printf("Red\n");
            break;
        case COLOR_GREEN:
            printf("Green\n");
            break;
        case COLOR_BLUE:
            printf("Blue\n");
            break;
    }
}

/* Union test */
typedef union {
    int ival;
    float fval;
    char *sval;
} Value;

void set_int(Value *v, int val) {
    v->ival = val;
}
